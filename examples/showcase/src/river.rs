//! The showcase's river (issue #49): one course, used by the asset
//! generator (`cargo xtask gen-assets` builds the carved terrain and the
//! water ribbon from it) and by the game (the ground height the hero and
//! the wading robot walk on, and where the water is). The meshes and the
//! collision can't disagree: they come from the same numbers.
//!
//! The river rises at a spring by the windmill (west), winds through the
//! courtyard in an S, passes under the bridge and opens into a delta by the
//! eastern trees, flowing off the edge of the world.
//!
//! Cross-section, `d` = distance from the centerline, `hw` = the half
//! width there:
//!
//! ```text
//!   ground (y 0) ___                               ___ ground
//!                   \  slope (BANK_RUN wide)      /
//!   water (WATER_Y) ~~\~~~~~~~~~~~~~~~~~~~~~~~~~/~~  ribbon to hw + RIBBON_OVER
//!                      \_______ bed (BED_Y) ___/
//!                      hw - BED_IN        hw - BED_IN
//! ```

use trino_core::Vec3;

/// A point of the course: centerline `(x, z)` and the channel's half width.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CoursePoint {
    pub x: f32,
    pub z: f32,
    pub half_width: f32,
}

const fn p(x: f32, z: f32, half_width: f32) -> CoursePoint {
    CoursePoint { x, z, half_width }
}

/// The centerline, upstream (the spring) to downstream (the world's east
/// edge): 17 rings, 16 sections (the water ribbon's lengthwise resolution).
pub const COURSE: [CoursePoint; 17] = [
    p(-12.0, -3.20, 0.55),
    p(-10.0, -2.09, 1.07),
    p(-8.0, -1.08, 1.23),
    p(-6.0, -0.29, 1.40),
    p(-4.0, 0.22, 1.40),
    p(-2.0, 0.40, 1.40),
    p(0.0, 0.22, 1.40),
    p(2.0, -0.29, 1.40),
    p(4.0, -1.08, 1.40),
    p(6.0, -2.09, 1.40),
    p(8.0, -3.20, 1.40),
    p(10.0, -4.31, 1.40),
    p(12.0, -5.32, 1.85),
    p(14.0, -6.11, 2.30),
    p(16.0, -6.62, 2.75),
    p(18.0, -6.80, 3.20),
    p(20.0, -6.62, 3.65),
];

/// Height of the river bed.
pub const BED_Y: f32 = -0.55;
/// Height of the water's surface at rest (the swell moves it a few cm).
pub const WATER_Y: f32 = -0.18;
/// The flat bed stops this far inside the half width.
pub const BED_IN: f32 = 0.35;
/// Horizontal run of the bank's slope (from the bed's edge to the ground).
pub const BANK_RUN: f32 = 0.9;
/// The water ribbon reaches this far past the half width, under the slope:
/// its edge stays buried even at a wave's crest.
pub const RIBBON_OVER: f32 = 0.4;

/// How far upstream of the first ring the spring basin's floor rises back
/// to the ground (the mesh fans the first cross-section to an apex there).
pub const SPRING_RUN: f32 = 1.2;

/// The bridge: centered on `BRIDGE_X`, spanning z from `BRIDGE_Z0` to
/// `BRIDGE_Z1`, `BRIDGE_HALF_W` to each side, its deck rising to
/// `BRIDGE_RISE` at mid-span (a parabola from the ground at both ends).
pub const BRIDGE_X: f32 = -4.6;
pub const BRIDGE_Z0: f32 = -2.2;
pub const BRIDGE_Z1: f32 = 2.6;
pub const BRIDGE_HALF_W: f32 = 0.8;
pub const BRIDGE_RISE: f32 = 0.55;
/// Deck thickness (the plank's top is the walking surface).
pub const BRIDGE_DECK: f32 = 0.12;

/// Stepping stones: center `(x, z)`, radius and the top's height.
pub const STONES: [(f32, f32, f32, f32); 4] = [
    (1.7, -1.55, 0.42, -0.04),
    (2.25, -0.75, 0.40, -0.02),
    (1.75, 0.10, 0.42, -0.05),
    (2.3, 0.95, 0.40, -0.03),
];

/// Where a point is, relative to the course: the lateral distance from the
/// centerline, the half width there, the distance along the course from
/// the spring and the flow direction (unit, x z).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct CoursePos {
    pub d: f32,
    pub half_width: f32,
    pub along: f32,
    pub flow: (f32, f32),
}

/// The nearest point on the centerline to `(x, z)`, as a [`CoursePos`].
/// `None` before the spring (upstream of the first ring) or past the end.
pub fn locate(x: f32, z: f32) -> Option<CoursePos> {
    let mut best: Option<(f32, CoursePos)> = None;
    let mut along = 0.0;
    for i in 0..COURSE.len() - 1 {
        let (a, b) = (COURSE[i], COURSE[i + 1]);
        let (sx, sz) = (b.x - a.x, b.z - a.z);
        let len2 = sx * sx + sz * sz;
        let len = trino_core::math3d::sqrt(len2);
        let t = ((x - a.x) * sx + (z - a.z) * sz) / len2;
        let first = i == 0;
        let last = i == COURSE.len() - 2;
        if (t < 0.0 && first) || (t > 1.0 && last) {
            along += len;
            continue;
        }
        let t = t.clamp(0.0, 1.0);
        let (cx, cz) = (a.x + sx * t, a.z + sz * t);
        let (dx, dz) = (x - cx, z - cz);
        let d2 = dx * dx + dz * dz;
        if best.as_ref().is_none_or(|(b2, _)| d2 < *b2) {
            best = Some((
                d2,
                CoursePos {
                    d: trino_core::math3d::sqrt(d2),
                    half_width: a.half_width + (b.half_width - a.half_width) * t,
                    along: along + len * t,
                    flow: (sx / len, sz / len),
                },
            ));
        }
        along += len;
    }
    best.map(|(_, p)| p)
}

/// The carved terrain's height at lateral distance `d` from a centerline of
/// half width `hw` (the cross-section above).
pub fn profile(d: f32, hw: f32) -> f32 {
    let d = if d < 0.0 { -d } else { d };
    let bed = hw - BED_IN;
    if d <= bed {
        BED_Y
    } else if d >= bed + BANK_RUN {
        0.0
    } else {
        BED_Y * (1.0 - (d - bed) / BANK_RUN)
    }
}

/// The terrain's height at world `(x, z)`: the ground (0) carved by the
/// river channel. The upstream end is a rounded spring basin.
pub fn terrain_height(x: f32, z: f32) -> f32 {
    // Upstream of the first ring: the spring basin, its cross-section
    // shrinking to an apex `SPRING_RUN` upstream.
    let (a, b) = (COURSE[0], COURSE[1]);
    let (tx, tz) = (b.x - a.x, b.z - a.z);
    let len = trino_core::math3d::sqrt(tx * tx + tz * tz);
    let (tx, tz) = (tx / len, tz / len);
    let up = -((x - a.x) * tx + (z - a.z) * tz);
    if up > 0.0 {
        if up >= SPRING_RUN {
            return 0.0;
        }
        let k = 1.0 - up / SPRING_RUN;
        let d = (x - a.x) * -tz + (z - a.z) * tx;
        return profile(d / k, a.half_width) * k;
    }
    match locate(x, z) {
        Some(c) => profile(c.d, c.half_width),
        None => 0.0,
    }
}

/// The bridge deck's walking height at `(x, z)`, if `(x, z)` is on it.
pub fn bridge_height(x: f32, z: f32) -> Option<f32> {
    let dx = x - BRIDGE_X;
    if !(-BRIDGE_HALF_W..=BRIDGE_HALF_W).contains(&dx) || !(BRIDGE_Z0..=BRIDGE_Z1).contains(&z) {
        return None;
    }
    let t = (z - BRIDGE_Z0) / (BRIDGE_Z1 - BRIDGE_Z0) * 2.0 - 1.0;
    Some(BRIDGE_RISE * (1.0 - t * t))
}

/// The height a walker stands on at `(x, z)`: the terrain, a stepping
/// stone's top or the bridge deck, whichever is highest.
pub fn floor_height(x: f32, z: f32) -> f32 {
    let mut h = terrain_height(x, z);
    for (sx, sz, r, top) in STONES {
        let (dx, dz) = (x - sx, z - sz);
        if dx * dx + dz * dz < r * r && top > h {
            h = top;
        }
    }
    match bridge_height(x, z) {
        Some(b) if b > h => b,
        _ => h,
    }
}

/// Is `(x, z)` over the water (the channel, inside the waterline)?
pub fn over_water(x: f32, z: f32) -> bool {
    terrain_height(x, z) < WATER_Y
}

/// How deep a body standing at `feet` is in the water at `(x, z)`: 0 when
/// dry (on the bank, a stone or the bridge).
pub fn wading_depth(feet: Vec3) -> f32 {
    if !over_water(feet.x, feet.z) {
        return 0.0;
    }
    let depth = WATER_Y - feet.y;
    if depth > 0.0 { depth } else { 0.0 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_channel_is_carved_and_the_banks_are_flat() {
        // On the centerline: the bed, under water.
        for c in &COURSE[1..] {
            assert_eq!(terrain_height(c.x, c.z), BED_Y, "{c:?}");
            assert!(over_water(c.x, c.z));
        }
        // Far from it: flat ground.
        assert_eq!(terrain_height(0.0, -6.0), 0.0, "the spawn");
        assert_eq!(terrain_height(-1.0, 3.5), 0.0, "the coins");
        assert!(!over_water(0.0, -6.0));
        // The slope crosses the waterline inside the ribbon's reach.
        let hw = 1.4;
        let mut d = hw - BED_IN;
        while profile(d, hw) < WATER_Y {
            d += 0.01;
        }
        assert!(d < hw + RIBBON_OVER, "waterline at {d}");
        assert!(
            profile(hw + RIBBON_OVER, hw) > WATER_Y + 0.06,
            "ribbon edge buried"
        );
    }

    #[test]
    fn crossings_stand_above_the_water() {
        for (x, z, _, top) in STONES {
            assert!(over_water(x, z), "a stone stands in the river");
            assert_eq!(floor_height(x, z), top);
            assert!(top > WATER_Y);
        }
        let mid = (BRIDGE_Z0 + BRIDGE_Z1) * 0.5;
        assert!(over_water(BRIDGE_X, 0.2), "the bridge spans the river");
        assert!((floor_height(BRIDGE_X, mid) - BRIDGE_RISE).abs() < 1e-5);
        // Its ends meet the ground.
        assert!(bridge_height(BRIDGE_X, BRIDGE_Z0).unwrap().abs() < 1e-5);
        assert!(terrain_height(BRIDGE_X, BRIDGE_Z0) > -0.05);
        assert!(terrain_height(BRIDGE_X, BRIDGE_Z1) > -0.05);
    }

    #[test]
    fn wading_is_measured_from_the_surface() {
        let c = COURSE[6];
        assert!((wading_depth(Vec3::new(c.x, BED_Y, c.z)) - (WATER_Y - BED_Y)).abs() < 1e-6);
        assert_eq!(wading_depth(Vec3::new(0.0, 0.0, -6.0)), 0.0);
        let f = locate(c.x, c.z).unwrap().flow;
        assert!(f.0 > 0.9, "flows east: {f:?}");
    }
}
