//! Portal/cell visibility for interiors: a level is split into **cells**
//! (rooms, boxes in world space) joined by **portals** (doorways). From the
//! camera's cell, a cell is visible only if a chain of portals leads to it
//! *through the screen*: each portal's projected rectangle narrows the view
//! for the cells behind it. Everything in an invisible cell is skipped
//! before the T&L — the way to keep a building inside the N64's 4 000
//! triangles when distance LOD is not enough.
//!
//! Authored in scenes (`cell` / `portal` components); nodes carry the cell
//! they belong to ([`crate::scene::SceneNode::cell`], [`NO_CELL`] = always
//! drawn: the exterior, the sky). Fixed capacity, no allocation: up to
//! [`MAX_CELLS`] cells (a `u64` mask) and `P` portals.
//!
//! A cell can also name the asset groups its contents come from (issue #56):
//! [`CellGroups`] keeps those of the cells in view, and of the rooms one
//! doorway further, loaded.

use crate::math::{Vec2, Vec3};
use crate::physics::Aabb;
use crate::render3d::Camera3;
use crate::scene::SceneNode;
use crate::{GroupId, LoadState, Platform};

/// A node outside every cell: drawn whenever drawn at all.
pub const NO_CELL: u8 = 0xFF;
/// Cells per level (one bit each in the visibility mask).
pub const MAX_CELLS: usize = 64;

/// Whether a node in `cell` draws under the visibility mask `visible`.
#[inline]
pub fn in_view(cell: u8, visible: u64) -> bool {
    (cell as usize) >= MAX_CELLS || visible & (1u64 << cell) != 0
}

/// The visibility role of a scene node (see the module docs).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum VisVolume {
    /// A room: the world box (local half extents around the node origin).
    Cell { half: Vec3 },
    /// A doorway between cells `a` and `b` (indices in cell order).
    Portal { half: Vec3, a: u8, b: u8 },
}

/// One portal: its world box and the two cells it joins.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Portal {
    pub bounds: Aabb,
    pub a: u8,
    pub b: u8,
}

/// Screen rectangle (min, max) in pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
struct ScreenRect {
    min: Vec2,
    max: Vec2,
}

impl ScreenRect {
    fn intersect(&self, o: &ScreenRect) -> Option<ScreenRect> {
        let min = Vec2::new(self.min.x.max(o.min.x), self.min.y.max(o.min.y));
        let max = Vec2::new(self.max.x.min(o.max.x), self.max.y.min(o.max.y));
        (min.x < max.x && min.y < max.y).then_some(ScreenRect { min, max })
    }

    fn union(&self, o: &ScreenRect) -> ScreenRect {
        ScreenRect {
            min: Vec2::new(self.min.x.min(o.min.x), self.min.y.min(o.min.y)),
            max: Vec2::new(self.max.x.max(o.max.x), self.max.y.max(o.max.y)),
        }
    }

    fn contains(&self, o: &ScreenRect) -> bool {
        self.min.x <= o.min.x
            && self.min.y <= o.min.y
            && self.max.x >= o.max.x
            && self.max.y >= o.max.y
    }
}

/// The cells and portals of a level.
#[derive(Clone, Copy, Debug)]
pub struct Cells<const P: usize> {
    cells: [Aabb; MAX_CELLS],
    cell_count: usize,
    portals: [Portal; P],
    portal_count: usize,
}

impl<const P: usize> Default for Cells<P> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const P: usize> Cells<P> {
    pub const fn new() -> Self {
        Cells {
            cells: [Aabb::EMPTY; MAX_CELLS],
            cell_count: 0,
            portals: [Portal {
                bounds: Aabb::EMPTY,
                a: 0,
                b: 0,
            }; P],
            portal_count: 0,
        }
    }

    /// Build from scene nodes: `Cell` volumes in node order become cells
    /// 0, 1, 2…; `Portal` volumes join them (extra ones past capacity are
    /// dropped).
    /// Works on exported node slices and on `BakedScene::nodes()`.
    pub fn from_nodes<N: core::borrow::Borrow<SceneNode>>(
        nodes: impl IntoIterator<Item = N>,
    ) -> Self {
        Self::from_volumes(nodes.into_iter().filter_map(|n| {
            let n = n.borrow();
            n.vis.map(|v| (n.position(), v))
        }))
    }

    /// Build from (world center, volume) pairs, in cell order.
    pub fn from_volumes(volumes: impl IntoIterator<Item = (Vec3, VisVolume)>) -> Self {
        let mut c = Self::new();
        for (center, v) in volumes {
            match v {
                VisVolume::Cell { half } => {
                    c.add_cell(Aabb::from_center_half(center, half));
                }
                VisVolume::Portal { half, a, b } => {
                    c.add_portal(Portal {
                        bounds: Aabb::from_center_half(center, half),
                        a,
                        b,
                    });
                }
            }
        }
        c
    }

    pub fn add_cell(&mut self, bounds: Aabb) -> Option<u8> {
        if self.cell_count == MAX_CELLS {
            return None;
        }
        self.cells[self.cell_count] = bounds;
        self.cell_count += 1;
        Some((self.cell_count - 1) as u8)
    }

    pub fn add_portal(&mut self, portal: Portal) -> bool {
        if self.portal_count == P {
            return false;
        }
        self.portals[self.portal_count] = portal;
        self.portal_count += 1;
        true
    }

    pub fn cell_count(&self) -> usize {
        self.cell_count
    }

    pub fn portal_count(&self) -> usize {
        self.portal_count
    }

    /// The first cell containing `p`.
    pub fn cell_of(&self, p: Vec3) -> Option<u8> {
        self.cells[..self.cell_count]
            .iter()
            .position(|c| c.contains(p))
            .map(|i| i as u8)
    }

    /// The cells one portal away from those in `mask` (and not in it): the
    /// rooms a step through a doorway brings into view.
    pub fn neighbors(&self, mask: u64) -> u64 {
        let mut out = 0;
        for p in &self.portals[..self.portal_count] {
            if (p.a as usize) >= MAX_CELLS || (p.b as usize) >= MAX_CELLS {
                continue;
            }
            if mask & (1u64 << p.a) != 0 {
                out |= 1u64 << p.b;
            }
            if mask & (1u64 << p.b) != 0 {
                out |= 1u64 << p.a;
            }
        }
        out & !mask
    }

    /// Bit mask of cells visible from `camera` on a `screen`-sized view.
    /// A camera outside every cell (exterior) sees all of them; a level
    /// without cells is all-visible.
    pub fn visible(&self, camera: &Camera3, screen: Vec2) -> u64 {
        let Some(start) = self.cell_of(camera.eye) else {
            return u64::MAX;
        };
        let view = camera.view();
        let half_fov = camera.fov_y * 0.5;
        let focal = crate::math3d::cos(half_fov) / crate::math3d::sin(half_fov) * screen.y * 0.5;
        let full = ScreenRect {
            min: Vec2::ZERO,
            max: screen,
        };
        let mut mask: u64 = 1u64 << start;
        // The screen area each reached cell has been explored through. A
        // cell reached again through a view that is not inside it (a
        // second, wider doorway) is explored again with the union, so
        // rooms seen only through that doorway are not lost.
        let mut seen = [full; MAX_CELLS];
        // Depth-first over (cell, clip rect); fixed stack and step budget.
        // Running out of either returns everything visible (conservative).
        const STACK: usize = 32;
        const STEPS: usize = 256;
        let mut stack = [(0u8, full); STACK];
        stack[0] = (start, full);
        let mut top = 1;
        let mut steps = 0;
        while top > 0 {
            steps += 1;
            if steps > STEPS {
                return u64::MAX;
            }
            top -= 1;
            let (cell, clip) = stack[top];
            for p in &self.portals[..self.portal_count] {
                let next = if p.a == cell {
                    p.b
                } else if p.b == cell {
                    p.a
                } else {
                    continue;
                };
                if next as usize >= self.cell_count {
                    continue;
                }
                let Some(rect) = project(&view, focal, screen, &p.bounds) else {
                    continue; // behind the camera
                };
                let Some(narrow) = rect.intersect(&clip) else {
                    continue; // off screen through this chain
                };
                let bit = 1u64 << next;
                let explore = if mask & bit == 0 {
                    mask |= bit;
                    seen[next as usize] = narrow;
                    narrow
                } else if seen[next as usize].contains(&narrow) {
                    continue; // nothing new through this chain
                } else {
                    seen[next as usize] = seen[next as usize].union(&narrow);
                    seen[next as usize]
                };
                if top == STACK {
                    return u64::MAX;
                }
                stack[top] = (next, explore);
                top += 1;
            }
        }
        mask
    }
}

/// Asset groups streamed by cell (issue #56, the scene's `cell(groups: …)`):
/// each cell names the groups its contents come from, and
/// [`stream`](Self::stream) keeps loaded those of the cells in view and of
/// their portal neighbours (a room starts loading a doorway before it
/// shows), and unloads a group once no such cell has named it for `linger`
/// frames (a camera standing in a doorway does not reload a room every
/// frame). Groups the level `requires` as a whole stay out of it. Up to `G`
/// (at most 64) distinct groups; fixed size, no allocation.
#[derive(Clone, Debug)]
pub struct CellGroups<const G: usize> {
    groups: [GroupId; G],
    count: usize,
    /// Per cell, its groups as bits (bit i = `groups[i]`).
    cells: [u64; MAX_CELLS],
    /// Per group: frames since a wanted cell last named it; `NOT_HELD`
    /// when this streamer has not asked for it (or let it go).
    idle: [u16; G],
    linger: u16,
}

const NOT_HELD: u16 = u16::MAX;

impl<const G: usize> CellGroups<G> {
    pub const fn new(linger: u16) -> Self {
        const { assert!(G <= 64, "a u64 holds the group bits") };
        CellGroups {
            groups: [GroupId(0); G],
            count: 0,
            cells: [0; MAX_CELLS],
            idle: [NOT_HELD; G],
            linger: if linger == NOT_HELD {
                NOT_HELD - 1
            } else {
                linger
            },
        }
    }

    /// `cell`'s contents come from `group`. False when the cell is past
    /// [`MAX_CELLS`] or `G` groups are taken.
    pub fn add(&mut self, cell: u8, group: GroupId) -> bool {
        if cell as usize >= MAX_CELLS {
            return false;
        }
        let i = match self.groups[..self.count].iter().position(|g| *g == group) {
            Some(i) => i,
            None if self.count < G => {
                self.groups[self.count] = group;
                self.count += 1;
                self.count - 1
            }
            None => return false,
        };
        self.cells[cell as usize] |= 1u64 << i;
        true
    }

    /// The distinct groups the cells name, in first-named order.
    pub fn groups(&self) -> &[GroupId] {
        &self.groups[..self.count]
    }

    /// The groups the cells in `mask` name, as bits over [`groups`](Self::groups).
    pub fn wanted(&self, mask: u64) -> u64 {
        let mut bits = 0;
        let mut m = mask;
        while m != 0 {
            let cell = m.trailing_zeros() as usize;
            m &= m - 1;
            bits |= self.cells[cell];
        }
        bits
    }

    /// Whether every group the cells in `mask` name is `Ready` on `platform`.
    pub fn ready<P: Platform + ?Sized>(&self, platform: &P, mask: u64) -> bool {
        self.each(self.wanted(mask))
            .all(|g| platform.group_state(g) == LoadState::Ready)
    }

    /// Once a frame, after the visibility pass: load the groups of the cells
    /// in `visible` and of their neighbours in `cells`, unload those that
    /// lingered unwanted. True when the visible cells' groups are all
    /// `Ready` (their contents draw); a room still loading draws without
    /// them, as a group's assets do until it is ready.
    pub fn stream<P: Platform + ?Sized, const C: usize>(
        &mut self,
        platform: &mut P,
        cells: &Cells<C>,
        visible: u64,
    ) -> bool {
        let want = self.wanted(visible | cells.neighbors(visible));
        for i in 0..self.count {
            if want & (1u64 << i) != 0 {
                self.idle[i] = 0;
                platform.load_group(self.groups[i]);
            } else if self.idle[i] != NOT_HELD {
                self.idle[i] += 1;
                if self.idle[i] > self.linger {
                    platform.unload_group(self.groups[i]);
                    self.idle[i] = NOT_HELD;
                }
            }
        }
        self.ready(platform, visible)
    }

    /// Unload every group this streamer holds (leaving the level).
    pub fn release<P: Platform + ?Sized>(&mut self, platform: &mut P) {
        for i in 0..self.count {
            if self.idle[i] != NOT_HELD {
                platform.unload_group(self.groups[i]);
                self.idle[i] = NOT_HELD;
            }
        }
    }

    /// Whether this streamer holds `group` (asked for it, not yet let go).
    pub fn holds(&self, group: GroupId) -> bool {
        self.groups[..self.count]
            .iter()
            .zip(&self.idle)
            .any(|(g, idle)| *g == group && *idle != NOT_HELD)
    }

    fn each(&self, bits: u64) -> impl Iterator<Item = GroupId> + '_ {
        (0..self.count)
            .filter(move |i| bits & (1u64 << i) != 0)
            .map(|i| self.groups[i])
    }
}

/// Screen rectangle of a world box, `None` if entirely behind the camera.
/// A box crossing the near plane covers the whole screen (conservative).
fn project(view: &crate::math3d::Mat34, focal: f32, screen: Vec2, b: &Aabb) -> Option<ScreenRect> {
    const NEAR: f32 = 0.05;
    let center = screen * 0.5;
    let mut min = Vec2::new(f32::MAX, f32::MAX);
    let mut max = Vec2::new(f32::MIN, f32::MIN);
    let mut behind = 0;
    for i in 0..8 {
        let p = Vec3::new(
            if i & 1 == 0 { b.min.x } else { b.max.x },
            if i & 2 == 0 { b.min.y } else { b.max.y },
            if i & 4 == 0 { b.min.z } else { b.max.z },
        );
        let v = view.transform_point(p);
        if v.z <= NEAR {
            behind += 1;
            continue;
        }
        let s = Vec2::new(center.x + v.x * focal / v.z, center.y - v.y * focal / v.z);
        min = Vec2::new(min.x.min(s.x), min.y.min(s.y));
        max = Vec2::new(max.x.max(s.x), max.y.max(s.y));
    }
    match behind {
        8 => None,
        0 => Some(ScreenRect { min, max }),
        _ => Some(ScreenRect {
            min: Vec2::ZERO,
            max: screen,
        }),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Three 4x3x4 rooms in a row along +X (A: x 0..4, B: 4..8, C: 8..12)
    /// with 1 m wide doorways at z = 2 between them.
    fn level() -> Cells<8> {
        let mut c = Cells::new();
        for i in 0..3 {
            let x = i as f32 * 4.0;
            c.add_cell(Aabb::new(
                Vec3::new(x, 0.0, 0.0),
                Vec3::new(x + 4.0, 3.0, 4.0),
            ));
        }
        for (x, a, b) in [(4.0, 0, 1), (8.0, 1, 2)] {
            c.add_portal(Portal {
                bounds: Aabb::new(Vec3::new(x - 0.1, 0.0, 1.5), Vec3::new(x + 0.1, 2.0, 2.5)),
                a,
                b,
            });
        }
        c
    }

    fn cam(eye: Vec3, target: Vec3) -> Camera3 {
        Camera3 {
            eye,
            target,
            fov_y: 1.0,
        }
    }

    const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

    #[test]
    fn looking_through_the_doors_sees_the_chain() {
        let c = level();
        // In A at the door's height, looking down the doorways: all three.
        let m = c.visible(
            &cam(Vec3::new(1.0, 1.0, 2.0), Vec3::new(12.0, 1.0, 2.0)),
            SCREEN,
        );
        assert_eq!(m & 0b111, 0b111);
    }

    #[test]
    fn looking_away_sees_only_the_own_room() {
        let c = level();
        let m = c.visible(
            &cam(Vec3::new(2.0, 1.0, 2.0), Vec3::new(-5.0, 1.0, 2.0)),
            SCREEN,
        );
        assert_eq!(m & 0b111, 0b001);
    }

    #[test]
    fn a_narrow_view_through_a_door_can_still_hide_the_far_room() {
        let c = level();
        // In A near the side wall, looking diagonally at the A/B door: B is
        // visible through it, but C's door is outside that narrowed view.
        let m = c.visible(
            &cam(Vec3::new(3.0, 1.0, 0.3), Vec3::new(4.0, 1.0, 2.0)),
            SCREEN,
        );
        assert_eq!(m & 0b011, 0b011, "own room + the room behind the door");
        assert_eq!(m & 0b100, 0, "the far room is hidden: {m:b}");
    }

    #[test]
    fn the_portal_rect_narrows_the_view_for_rooms_behind_it() {
        // Same rooms, but the B/C door is off to the side (z 0..0.5).
        let mut c: Cells<8> = Cells::new();
        for i in 0..3 {
            let x = i as f32 * 4.0;
            c.add_cell(Aabb::new(
                Vec3::new(x, 0.0, 0.0),
                Vec3::new(x + 4.0, 3.0, 4.0),
            ));
        }
        c.add_portal(Portal {
            bounds: Aabb::new(Vec3::new(3.9, 0.0, 1.5), Vec3::new(4.1, 2.0, 2.5)),
            a: 0,
            b: 1,
        });
        c.add_portal(Portal {
            bounds: Aabb::new(Vec3::new(7.9, 0.0, 0.0), Vec3::new(8.1, 2.0, 0.5)),
            a: 1,
            b: 2,
        });
        let view = cam(Vec3::new(1.0, 1.0, 2.0), Vec3::new(9.0, 1.0, 2.0));
        // The far door is on screen (~12 deg off axis, the half fov is
        // ~36 deg), but outside the A/B doorway's ~9.5 deg slice.
        let m = c.visible(&view, SCREEN);
        assert_eq!(m & 0b111, 0b011, "{m:b}");
        // Step into B, facing the side door: C shows, A is behind.
        let m = c.visible(
            &cam(Vec3::new(5.0, 1.0, 2.0), Vec3::new(8.0, 1.0, 0.5)),
            SCREEN,
        );
        assert_eq!(m & 0b111, 0b110, "{m:b}");
    }

    #[test]
    fn a_second_wider_doorway_is_explored_too() {
        // A and B share two doorways: a slit at z 0.1..0.2 (listed first,
        // so B is reached through it first) and a wide door at z 1..3.
        // C lies behind B, seen from A only through the wide door.
        let mut c: Cells<8> = Cells::new();
        for i in 0..3 {
            let x = i as f32 * 4.0;
            c.add_cell(Aabb::new(
                Vec3::new(x, 0.0, 0.0),
                Vec3::new(x + 4.0, 3.0, 4.0),
            ));
        }
        let door = |x: f32, z0: f32, z1: f32, a, b| Portal {
            bounds: Aabb::new(Vec3::new(x - 0.1, 0.0, z0), Vec3::new(x + 0.1, 2.0, z1)),
            a,
            b,
        };
        c.add_portal(door(4.0, 0.1, 0.2, 0, 1));
        c.add_portal(door(4.0, 1.0, 3.0, 0, 1));
        c.add_portal(door(8.0, 1.5, 2.5, 1, 2));
        let m = c.visible(
            &cam(Vec3::new(1.0, 1.0, 2.0), Vec3::new(12.0, 1.0, 2.0)),
            SCREEN,
        );
        assert_eq!(m & 0b111, 0b111, "{m:b}");
    }

    #[test]
    fn neighbors_are_one_doorway_away() {
        let c = level();
        assert_eq!(c.neighbors(0b001), 0b010);
        assert_eq!(c.neighbors(0b010), 0b101);
        assert_eq!(c.neighbors(0b011), 0b100);
        assert_eq!(c.neighbors(0), 0);
    }

    /// Groups load as asked (Ready at once) and go when unloaded.
    #[derive(Default)]
    struct Streams {
        loaded: std::vec::Vec<GroupId>,
        loads: usize,
        unloads: usize,
    }

    impl Platform for Streams {
        fn now_micros(&self) -> u64 {
            0
        }
        fn log(&self, _: &str) {}
        fn load_group(&mut self, g: GroupId) -> LoadState {
            if !self.loaded.contains(&g) {
                self.loaded.push(g);
                self.loads += 1;
            }
            LoadState::Ready
        }
        fn unload_group(&mut self, g: GroupId) {
            self.loaded.retain(|x| *x != g);
            self.unloads += 1;
        }
        fn group_state(&self, g: GroupId) -> LoadState {
            if self.loaded.contains(&g) {
                LoadState::Ready
            } else {
                LoadState::Unloaded
            }
        }
    }

    #[test]
    fn cell_groups_follow_the_view_and_linger() {
        let c = level();
        let (a, b, kitchen) = (
            GroupId::from_name("hall"),
            GroupId::from_name("study"),
            GroupId::from_name("kitchen"),
        );
        let mut s: CellGroups<4> = CellGroups::new(3);
        assert!(s.add(0, a) && s.add(1, b) && s.add(2, kitchen) && s.add(2, a));
        assert!(!s.add(64, a), "past the mask");
        assert_eq!(s.groups(), &[a, b, kitchen]);
        assert_eq!(s.wanted(0b100), 0b101, "the kitchen names two");

        let mut p = Streams::default();
        // In the hall: it and the study (a doorway away) load, not the kitchen.
        assert!(s.stream(&mut p, &c, 0b001));
        assert!(p.loaded.contains(&a) && p.loaded.contains(&b));
        assert!(!p.loaded.contains(&kitchen) && !s.holds(kitchen));
        // Into the kitchen: the hall stays (it names `a` too), the kitchen
        // loads, the study (its neighbour) stays wanted.
        assert!(s.stream(&mut p, &c, 0b100));
        assert!(p.loaded.contains(&kitchen) && p.loaded.contains(&b));
        // Back in the hall: the kitchen lingers `linger` frames, then goes.
        for _ in 0..3 {
            s.stream(&mut p, &c, 0b001);
            assert!(p.loaded.contains(&kitchen), "lingers");
        }
        s.stream(&mut p, &c, 0b001);
        assert!(!p.loaded.contains(&kitchen) && !s.holds(kitchen));
        assert_eq!(p.unloads, 1);
        // A flicker back within the linger reloads nothing.
        let loads = p.loads;
        s.stream(&mut p, &c, 0b001);
        assert_eq!(p.loads, loads);

        s.release(&mut p);
        assert!(p.loaded.is_empty());
        assert!(!s.ready(&p, 0b001));
    }

    #[test]
    fn a_room_still_loading_is_not_ready() {
        struct Slow;
        impl Platform for Slow {
            fn now_micros(&self) -> u64 {
                0
            }
            fn log(&self, _: &str) {}
            fn load_group(&mut self, _: GroupId) -> LoadState {
                LoadState::Loading { progress: 40 }
            }
            fn group_state(&self, _: GroupId) -> LoadState {
                LoadState::Loading { progress: 40 }
            }
        }
        let c = level();
        let mut s: CellGroups<2> = CellGroups::new(0);
        s.add(1, GroupId::from_name("study"));
        assert!(!s.stream(&mut Slow, &c, 0b010));
        // A cell without groups is ready at once, its loading neighbour
        // or not.
        assert!(s.stream(&mut Slow, &c, 0b100));
    }

    #[test]
    fn in_view_masks() {
        assert!(in_view(NO_CELL, 0));
        assert!(in_view(3, 0b1000) && !in_view(2, 0b1000));
        assert!(in_view(63, 1 << 63));
    }

    #[test]
    fn exterior_and_empty_levels_see_everything() {
        let c = level();
        assert_eq!(
            c.visible(&cam(Vec3::new(-10.0, 1.0, 2.0), Vec3::ZERO), SCREEN),
            u64::MAX
        );
        let none: Cells<1> = Cells::new();
        assert_eq!(none.visible(&cam(Vec3::ZERO, Vec3::ONE), SCREEN), u64::MAX);
        assert_eq!(c.cell_of(Vec3::new(5.0, 1.0, 1.0)), Some(1));
        assert_eq!(c.cell_of(Vec3::new(50.0, 1.0, 1.0)), None);
    }
}
