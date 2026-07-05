//! AABB-vs-tilemap collision: the movement core for 2D platformers.
//!
//! Axis-separated (X then Y per substep), with the delta split into
//! substeps no larger than half a tile so fast objects never tunnel
//! through walls. Pure `f32` arithmetic — deterministic across PC, N64
//! and 3DS (no transcendentals, no platform intrinsics).

use crate::math::Vec2;
use crate::tilemap::{TILE_SIZE, Tilemap};

/// What [`move_and_collide`] hit along the way.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct MoveResult {
    /// Final top-left position after collision resolution.
    pub pos: Vec2,
    /// A downward move was blocked (standing on something).
    pub on_ground: bool,
    /// An upward move was blocked (bonked a ceiling).
    pub hit_ceiling: bool,
    /// A horizontal move was blocked.
    pub hit_wall: bool,
}

/// Move an AABB (top-left `pos`, `size`) by `delta` through the map's solid
/// tiles, sliding along surfaces. Positions resolve flush against tile
/// faces, so a body that landed reports `on_ground` again every frame
/// gravity pushes it down.
pub fn move_and_collide(map: &Tilemap, pos: Vec2, size: Vec2, delta: Vec2) -> MoveResult {
    let mut result = MoveResult {
        pos,
        ..Default::default()
    };

    // Substep so no single step exceeds half a tile on either axis. The
    // step count is capped, so the total displacement is too (64 * 8 = 512
    // px per call) — far beyond any sane per-frame speed, and clamping
    // beats tunneling.
    const MAX_STEPS: f32 = 64.0;
    let max_step = TILE_SIZE * 0.5;
    let longest = if delta.x.abs() > delta.y.abs() {
        delta.x.abs()
    } else {
        delta.y.abs()
    };
    let (steps, delta) = if longest <= max_step {
        (1u32, delta)
    } else if longest <= max_step * MAX_STEPS {
        // ceil(longest / max_step), in integer math to stay deterministic.
        (((longest / max_step) as u32) + 1, delta)
    } else {
        (MAX_STEPS as u32, delta * (max_step * MAX_STEPS / longest))
    };
    let step = delta * (1.0 / steps as f32);

    for _ in 0..steps {
        step_axis_x(map, &mut result, size, step.x);
        step_axis_y(map, &mut result, size, step.y);
    }
    result
}

/// Tile range [lo, hi] overlapped by the half-open span [start, start+len).
#[inline]
fn tile_span(start: f32, len: f32) -> (i32, i32) {
    let lo = floor_div(start);
    // A hair inside the far edge: the span is half-open.
    let hi = floor_div(start + len - 0.001);
    (lo, hi)
}

#[inline]
fn floor_div(v: f32) -> i32 {
    let t = v / TILE_SIZE;
    // f32 `floor` without libm/std: truncation adjusts for negatives.
    let i = t as i32;
    if t < i as f32 { i - 1 } else { i }
}

fn step_axis_x(map: &Tilemap, result: &mut MoveResult, size: Vec2, dx: f32) {
    if dx == 0.0 {
        return;
    }
    let new_x = result.pos.x + dx;
    let (ty_lo, ty_hi) = tile_span(result.pos.y, size.y);
    if dx > 0.0 {
        let leading = floor_div(new_x + size.x - 0.001);
        for ty in ty_lo..=ty_hi {
            if map.is_solid(leading, ty) {
                result.pos.x = leading as f32 * TILE_SIZE - size.x;
                result.hit_wall = true;
                return;
            }
        }
    } else {
        let leading = floor_div(new_x);
        for ty in ty_lo..=ty_hi {
            if map.is_solid(leading, ty) {
                result.pos.x = (leading + 1) as f32 * TILE_SIZE;
                result.hit_wall = true;
                return;
            }
        }
    }
    result.pos.x = new_x;
}

fn step_axis_y(map: &Tilemap, result: &mut MoveResult, size: Vec2, dy: f32) {
    if dy == 0.0 {
        return;
    }
    let new_y = result.pos.y + dy;
    let (tx_lo, tx_hi) = tile_span(result.pos.x, size.x);
    if dy > 0.0 {
        let leading = floor_div(new_y + size.y - 0.001);
        for tx in tx_lo..=tx_hi {
            if map.is_solid(tx, leading) {
                result.pos.y = leading as f32 * TILE_SIZE - size.y;
                result.on_ground = true;
                return;
            }
        }
    } else {
        let leading = floor_div(new_y);
        for tx in tx_lo..=tx_hi {
            if map.is_solid(tx, leading) {
                result.pos.y = (leading + 1) as f32 * TILE_SIZE;
                result.hit_ceiling = true;
                return;
            }
        }
    }
    result.pos.y = new_y;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tilemap::Tilemap;

    // 8x6 tiles: floor at row 5, wall at column 5 (rows 3-4), ceiling
    // block at (2,1).
    const LEVEL: &str = "\
........\n\
..B.....\n\
........\n\
.....B..\n\
.....B..\n\
########";

    fn map() -> Tilemap<'static> {
        Tilemap::parse(LEVEL).unwrap()
    }

    const SIZE: Vec2 = Vec2::new(14.0, 14.0);

    #[test]
    fn falls_and_lands_flush_on_floor() {
        let m = map();
        // x=50 spans tile column 3 only (clear of the block at column 2).
        // Floor top edge is at y = 5*16 = 80; body height 14 -> rests at 66.
        let r = move_and_collide(&m, Vec2::new(50.0, 10.0), SIZE, Vec2::new(0.0, 100.0));
        assert!(r.on_ground);
        assert_eq!(r.pos.y, 80.0 - SIZE.y);
        assert_eq!(r.pos.x, 50.0);
    }

    #[test]
    fn big_fall_does_not_tunnel_through_floor() {
        let m = map();
        // 10_000 px exceeds the substep budget: the delta clamps (never
        // tunnels), so it takes a few calls to cover any distance.
        let mut pos = Vec2::new(50.0, 0.0);
        let mut landed = false;
        for _ in 0..25 {
            let r = move_and_collide(&m, pos, SIZE, Vec2::new(0.0, 10_000.0));
            pos = r.pos;
            if r.on_ground {
                landed = true;
                break;
            }
        }
        assert!(landed);
        assert_eq!(pos.y, 80.0 - SIZE.y);
    }

    #[test]
    fn wall_blocks_and_slides() {
        let m = map();
        // Standing on the floor (y=66), running right into the wall at
        // column 5 (x = 5*16 = 80)... but the wall spans rows 3-4
        // (y 48..80), which overlaps the body at y=66.
        let r = move_and_collide(&m, Vec2::new(40.0, 66.0), SIZE, Vec2::new(100.0, 0.0));
        assert!(r.hit_wall);
        assert_eq!(r.pos.x, 80.0 - SIZE.x);
        assert_eq!(r.pos.y, 66.0);
    }

    #[test]
    fn moving_left_stops_at_wall_right_face() {
        let m = map();
        let r = move_and_collide(&m, Vec2::new(110.0, 66.0), SIZE, Vec2::new(-100.0, 0.0));
        assert!(r.hit_wall);
        assert_eq!(r.pos.x, 6.0 * 16.0);
    }

    #[test]
    fn ceiling_stops_upward_motion() {
        let m = map();
        // Block at (2,1): y 16..32. Jump up under it from x=34 (spans
        // tiles 2-2 horizontally with width 14).
        let r = move_and_collide(&m, Vec2::new(34.0, 50.0), SIZE, Vec2::new(0.0, -40.0));
        assert!(r.hit_ceiling);
        assert_eq!(r.pos.y, 32.0);
    }

    #[test]
    fn diagonal_slides_along_floor() {
        let m = map();
        let r = move_and_collide(&m, Vec2::new(10.0, 60.0), SIZE, Vec2::new(20.0, 40.0));
        assert!(r.on_ground);
        assert_eq!(r.pos.y, 80.0 - SIZE.y);
        // X unaffected by the floor hit (substep float accumulation only).
        assert!((r.pos.x - 30.0).abs() < 0.001, "x = {}", r.pos.x);
    }

    #[test]
    fn free_movement_is_exact() {
        let m = map();
        let r = move_and_collide(&m, Vec2::new(10.0, 10.0), SIZE, Vec2::new(5.0, 3.0));
        assert_eq!(r.pos, Vec2::new(15.0, 13.0));
        assert!(!r.on_ground && !r.hit_wall && !r.hit_ceiling);
    }
}
