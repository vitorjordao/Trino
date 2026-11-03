//! Physics 2.1 (issue #115): physics per cell and the baked grid section.
//!
//! - A [`CellWorld`] answers what a scan of its loaded cells' solids
//!   answers (global indices, lowest on ties, ascending overlaps), for any
//!   mask of loaded cells.
//! - A body walks from one cell onto the next only while it is loaded, keeps
//!   its `standing_on` when other cells stream, and falls when its own
//!   floor unloads (unless the game keeps the cells under it loaded).
//! - Triggers across a stream: no Exit when a trigger's cell unloads under
//!   its body; a Stay when it comes back with the body inside, an Exit if
//!   the body left meanwhile.
//! - The section round-trips: the decoded grids are the built ones, bit
//!   for bit.

use trino_core::physics::{
    self, BakedPhysics, CastShape, CellPart, CellWorld, CharacterController, ControllerConfig,
    Solid, TriggerEvent, TriggerPhase, TriggerTracker, V3, build_parts, grids_size, part_count,
    raycast_solids, section_bytes, shape_cast_solids, write_section,
};
use trino_core::vis::NO_CELL;
use trino_core::{Aabb, LAYER_ALL, Vec3};

fn v(x: f32, y: f32, z: f32) -> Vec3 {
    Vec3::new(x, y, z)
}

struct Rng(u32);
impl Rng {
    fn next(&mut self) -> f32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        (self.0 >> 8) as f32 / (1u32 << 24) as f32
    }
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + self.next() * (hi - lo)
    }
}

const BLANK: CellPart<'static> = CellPart {
    cell: 0,
    base: 0,
    grid: None,
    trigger_base: 0,
    triggers: &[],
};

/// Five cells of 80 m in a row (and some solids in no cell), 400 solids.
fn streamed() -> (Vec<Solid>, Vec<u8>) {
    let mut r = Rng(0x0BAD_F00D);
    let mut out: Vec<(u8, Solid)> = (0..400)
        .map(|i| {
            let cell = if i % 17 == 0 { NO_CELL } else { (i % 5) as u8 };
            let x0 = cell.min(4) as f32 * 80.0;
            let c = v(
                r.range(x0, x0 + 80.0),
                r.range(0.0, 6.0),
                r.range(-40.0, 40.0),
            );
            let h = v(r.range(0.5, 5.0), r.range(0.5, 3.0), r.range(0.5, 5.0));
            let b = Aabb::from_center_half(c, h);
            let s = match i % 7 {
                0 => Solid::ramp(b, 1 << (i % 3), physics::RampDir::PosZ),
                1 => Solid::one_way(b, 1),
                _ => Solid::new(b, 1 << (i % 3)),
            };
            (cell, s)
        })
        .collect();
    out.sort_by_key(|(c, _)| *c);
    out.into_iter().map(|(c, s)| (s, c)).unzip()
}

fn parts<'a>(
    solids: &'a [Solid],
    cells: &'a [u8],
    triggers: &'a [Solid],
    tcells: &'a [u8],
    u16s: &'a mut Vec<u16>,
    parts: &'a mut Vec<CellPart<'a>>,
) -> &'a [CellPart<'a>] {
    u16s.resize(grids_size(solids, cells, 0.0).unwrap(), 0);
    parts.resize(part_count(cells, tcells), BLANK);
    build_parts(solids, cells, triggers, tcells, 0.0, u16s, parts).unwrap()
}

/// The loaded solids in global order, and their global indices.
fn loaded(solids: &[Solid], cells: &[u8], mask: u64) -> (Vec<Solid>, Vec<usize>) {
    solids
        .iter()
        .zip(cells)
        .enumerate()
        .filter(|(_, (_, c))| **c == NO_CELL || mask & (1 << **c) != 0)
        .map(|(i, (s, _))| (*s, i))
        .unzip()
}

#[test]
fn cell_worlds_answer_what_a_scan_of_the_loaded_cells_answers() {
    let (solids, cells) = streamed();
    let (mut u, mut p) = (vec![], vec![]);
    let parts = parts(&solids, &cells, &[], &[], &mut u, &mut p);
    assert_eq!(parts.len(), 6);
    assert_eq!(parts.last().unwrap().cell, NO_CELL);
    let mut world = CellWorld::new(parts, &[], &[]).unwrap();
    let mut r = Rng(42);
    let mut hits = 0;
    for mask in [0u64, 0b1, 0b10110, 0b11111, 0b01001] {
        world.set_loaded(mask);
        let (subset, ids) = loaded(&solids, &cells, mask);
        for i in 0..200 {
            let o = v(
                r.range(-10.0, 410.0),
                r.range(0.0, 8.0),
                r.range(-45.0, 45.0),
            );
            let d = v(r.range(-1.0, 1.0), r.range(-0.3, 0.1), r.range(-1.0, 1.0)).normalized();
            let m = [LAYER_ALL, 1, 6][i % 3];
            let a = world.raycast(o, d, 120.0, m);
            let b = raycast_solids(o, d, 120.0, m, &subset).map(|mut h| {
                h.index = ids[h.index];
                h
            });
            assert_eq!(a, b, "mask {mask:b}");
            hits += a.is_some() as usize;
            let to = o + d * 25.0;
            let shape = CastShape::Capsule {
                radius: 0.4,
                height: 1.8,
            };
            let a = world.shape_cast(shape, o, to, m);
            let b = shape_cast_solids(shape, o, to, m, &subset).map(|mut h| {
                h.index = ids[h.index];
                h
            });
            assert_eq!(a, b);
            let q = Aabb::from_center_half(o, v(10.0, 3.0, 10.0));
            let (mut x, mut y) = ([0u16; 12], [0u16; 12]);
            let nx = world.overlap_aabb(&q, m, &mut x);
            let ny = physics::overlap_solids(&q, m, &subset, &mut y);
            assert_eq!(nx, ny);
            let want: Vec<u16> = y[..ny.min(12)]
                .iter()
                .map(|&k| ids[k as usize] as u16)
                .collect();
            assert_eq!(x[..nx.min(12)], want[..]);
            for &id in &x[..nx.min(12)] {
                assert!(world.solid(id as usize).is_some());
            }
        }
    }
    assert!(hits > 100, "{hits}");
}

/// Cell 0: a floor at x 0..10; cell 1: a floor at x 10..20, a trigger
/// over it; a wall in no cell at x 19 (on cell 1's floor).
fn rooms() -> (Vec<Solid>, Vec<u8>, Vec<Solid>, Vec<u8>) {
    let floor = |x0: f32| Solid::new(Aabb::new(v(x0, -1.0, -5.0), v(x0 + 10.0, 0.0, 5.0)), 1);
    let wall = Solid::new(Aabb::new(v(19.0, 0.0, -5.0), v(20.0, 3.0, 5.0)), 1);
    let pad = Solid::new(Aabb::new(v(12.0, 0.0, -2.0), v(18.0, 2.0, 2.0)), 1);
    (
        vec![floor(0.0), floor(10.0), wall],
        vec![0, 1, NO_CELL],
        vec![pad],
        vec![1],
    )
}

#[test]
fn bodies_collide_only_with_loaded_cells() {
    let (s, sc, t, tc) = rooms();
    let (mut u, mut p) = (vec![], vec![]);
    let parts = parts(&s, &sc, &t, &tc, &mut u, &mut p);
    let mut world = CellWorld::new(parts, &[], &[]).unwrap();
    let cfg = ControllerConfig::new(V3::new(0.3, 0.55, 0.3), 25.0, 20.0);
    let walk = |world: &CellWorld, c: &mut CharacterController<f32>, frames: usize, vx: f32| {
        for _ in 0..frames {
            c.vel.x = vx;
            c.step_in(1.0 / 60.0, world, &[]);
        }
    };
    // Only cell 0: walking east off its floor falls.
    world.set_loaded(0b01);
    let mut c = CharacterController::new(cfg, V3::new(5.0, 0.56, 0.0));
    walk(&world, &mut c, 30, 0.0);
    assert!(c.on_ground && c.standing_on == Some(0));
    walk(&world, &mut c, 120, 4.0);
    assert!(
        c.pos.y < -1.0,
        "fell past cell 1's unloaded floor: {:?}",
        c.pos
    );
    // Both loaded: it walks onto cell 1's floor (global index 1).
    world.set_loaded(0b11);
    let mut c = CharacterController::new(cfg, V3::new(5.0, 0.56, 0.0));
    walk(&world, &mut c, 120, 4.0);
    assert!(c.on_ground && c.standing_on == Some(1) && c.pos.x > 12.0);
    // Cell 0 streams out and back: standing_on stays cell 1's floor.
    world.set_loaded(0b10);
    walk(&world, &mut c, 10, 0.0);
    assert_eq!(c.standing_on, Some(1));
    world.set_loaded(0b11);
    walk(&world, &mut c, 10, 0.0);
    assert_eq!(c.standing_on, Some(1));
    // Its own floor unloads under it: it falls (a game pins it instead).
    let under = Aabb::from_center_half(c.pos.into(), v(0.3, 0.6, 0.3));
    assert_eq!(world.cells_under(&under), 0b10);
    world.set_loaded(0b01);
    walk(&world, &mut c, 3, 0.0);
    assert!(!c.on_ground && c.standing_on.is_none());
    // Pinned: what streams plus the cells under the body.
    let mut c = CharacterController::new(cfg, V3::new(15.0, 0.56, 0.0));
    world.set_loaded(0b11);
    walk(&world, &mut c, 5, 0.0);
    for _ in 0..30 {
        let streamed = 0b01; // cell 1 went out of view
        let under = Aabb::from_center_half(c.pos.into(), v(0.4, 0.7, 0.4));
        world.set_loaded(streamed | world.cells_under(&under));
        walk(&world, &mut c, 1, 0.0);
    }
    assert!(c.on_ground && c.standing_on == Some(1));
    // The always-loaded wall (no cell) stops it.
    world.set_loaded(0b11);
    walk(&world, &mut c, 200, 5.0);
    assert!((c.pos.x - 18.7).abs() < 1e-3 && c.on_ground, "{:?}", c.pos);
}

#[test]
fn triggers_hold_across_an_unload_and_come_back_as_stay() {
    let (s, sc, t, tc) = rooms();
    let (mut u, mut p) = (vec![], vec![]);
    let parts = parts(&s, &sc, &t, &tc, &mut u, &mut p);
    let mut world = CellWorld::new(parts, &[], &[]).unwrap();
    let mut tracker = TriggerTracker::<8>::new();
    let mut ev = [TriggerEvent {
        body: 0,
        trigger: 0,
        phase: TriggerPhase::Stay,
    }; 8];
    let mut frame = |world: &CellWorld, at: Vec3| {
        tracker.begin_frame();
        world.track(
            &mut tracker,
            3,
            &Aabb::from_center_half(at, v(0.3, 0.5, 0.3)),
            LAYER_ALL,
        );
        let n = tracker.end_frame_held(&mut ev, &|t| world.trigger_live(t));
        ev[..n]
            .iter()
            .map(|e| (e.trigger, e.phase))
            .collect::<Vec<_>>()
    };
    let inside = v(15.0, 0.5, 0.0);
    let outside = v(5.0, 0.5, 0.0);
    world.set_loaded(0b11);
    assert_eq!(frame(&world, inside), [(0, TriggerPhase::Enter)]);
    assert_eq!(frame(&world, inside), [(0, TriggerPhase::Stay)]);
    // Its cell unloads under the body: nothing (no Exit), frame after frame.
    world.set_loaded(0b01);
    assert_eq!(frame(&world, inside), []);
    assert_eq!(frame(&world, inside), []);
    // Back with the body still inside: a Stay, not a second Enter.
    world.set_loaded(0b11);
    assert_eq!(frame(&world, inside), [(0, TriggerPhase::Stay)]);
    // Unloaded again; the body leaves meanwhile; reload: an Exit.
    world.set_loaded(0b01);
    assert_eq!(frame(&world, outside), []);
    world.set_loaded(0b11);
    assert_eq!(frame(&world, outside), [(0, TriggerPhase::Exit)]);
    assert_eq!(frame(&world, outside), []);
}

#[test]
fn the_section_round_trips_bit_for_bit() {
    let (solids, cells) = streamed();
    let (tr, tcells) = {
        let (s, _, t, tc) = rooms();
        let _ = s;
        (t, tc)
    };
    let (mut u, mut p) = (vec![], vec![]);
    let built = parts(&solids, &cells, &tr, &tcells, &mut u, &mut p);
    let meshes = [(1u8, 7u32), (NO_CELL, 9)];
    let mut bytes = vec![0u8; section_bytes(built, meshes.len())];
    let n = write_section(built, &meshes, &mut bytes).unwrap();
    assert_eq!(n, bytes.len());
    let baked = BakedPhysics::parse(&bytes).unwrap();
    let (np, ns, nt, nm, nu) = baked.counts();
    assert_eq!((np, ns, nt, nm), (built.len(), solids.len(), 1, 2));
    assert_eq!(baked.mesh(0), (1, 7));
    let (mut ds, mut dt, mut du) = (
        vec![Solid::new(Aabb::EMPTY, 0); ns],
        vec![Solid::new(Aabb::EMPTY, 0); nt],
        vec![0u16; nu],
    );
    let mut dp = vec![BLANK; np];
    let decoded = baked.decode(&mut ds, &mut dt, &mut du, &mut dp).unwrap();
    for (a, b) in built.iter().zip(decoded) {
        assert_eq!(
            (a.cell, a.base, a.trigger_base),
            (b.cell, b.base, b.trigger_base)
        );
        assert_eq!(a.solids(), b.solids());
        assert_eq!(a.triggers, b.triggers);
        match (&a.grid, &b.grid) {
            (Some(x), Some(y)) => assert_eq!(x.raw(), y.raw()),
            (None, None) => {}
            _ => panic!("grid presence differs"),
        }
    }
    // Damage is refused, never a panic.
    for cut in [0, 10, 40, bytes.len() / 2, bytes.len() - 1] {
        assert!(BakedPhysics::parse(&bytes[..cut]).is_err());
    }
    let mut bad = bytes.clone();
    bad[4] ^= 0x40; // solid count
    assert!(BakedPhysics::parse(&bad).is_err());
}
