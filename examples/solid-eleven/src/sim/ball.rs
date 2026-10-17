//! Ball flight and roll at the sim's fixed tick: gravity, air drag,
//! Magnus curve from side spin, bounces that lose height and pace, turf
//! rolling resistance (lower on a wet pitch), the goal frame (posts and
//! bar) and the net.

use super::fx::{Fx, V2, V3};
use super::pitch::{self, BALL_R, End, GOAL_DEPTH, GOAL_H, GOAL_HALF_W, HALF_LEN, POST_R};

/// Ticks per second.
pub const HZ: i32 = 60;
/// One tick, in seconds.
pub const DT: Fx = Fx::ratio(1, HZ);

pub const GRAVITY: Fx = Fx::milli(9_810);
/// Quadratic air drag per meter (`a = -k |v| v`): a real ball is ~0.013.
pub const AIR_DRAG: Fx = Fx::milli(13);
/// Bounce: vertical speed kept, and horizontal speed kept by the turf.
pub const RESTITUTION: Fx = Fx::milli(560);
pub const BOUNCE_GRIP: Fx = Fx::milli(860);
/// Below this vertical speed a bounce settles into a roll.
pub const SETTLE_VY: Fx = Fx::milli(900);
/// Rolling resistance (m/s²) and grass drag (1/s) on a dry pitch.
pub const ROLL_DECEL: Fx = Fx::milli(600);
pub const GRASS_DRAG: Fx = Fx::milli(120);
/// Spin decays by this fraction per second.
pub const SPIN_DECAY: Fx = Fx::milli(700);
/// Lateral acceleration per unit of spin and of speed.
pub const MAGNUS: Fx = Fx::milli(1_000);
/// The posts and bar give back this much of the normal speed.
pub const POST_RESTITUTION: Fx = Fx::milli(600);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Ball {
    pub pos: V3,
    pub vel: V3,
    /// Side spin, rad/s-ish: positive curls the ball counter-clockwise
    /// seen from above (toward +perp of its direction).
    pub spin: Fx,
}

/// What happened to the ball this tick (sounds and stats).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct BallEvents {
    pub bounced: bool,
    pub hit_frame: bool,
    pub in_net: bool,
}

impl Ball {
    pub fn at_rest(p: V2) -> Ball {
        Ball {
            pos: V3::from_ground(p, BALL_R),
            vel: V3::ZERO,
            spin: Fx::ZERO,
        }
    }

    /// On the turf (rolling or still).
    #[inline]
    pub fn grounded(&self) -> bool {
        self.pos.y <= BALL_R + Fx::milli(5) && self.vel.y.abs() < SETTLE_VY
    }

    /// Ground speed.
    pub fn ground_speed(&self) -> Fx {
        self.vel.ground().len()
    }

    /// Advance one tick of free flight/roll. `wet` lowers the turf's grip.
    pub fn step(&mut self, wet: bool) -> BallEvents {
        let mut ev = BallEvents::default();
        let g = self.vel.ground();
        let speed = self.vel.len();

        // Air drag (quadratic) and Magnus curl, in the air or on the turf.
        let drag = AIR_DRAG.mul(speed).mul(DT);
        self.vel = self.vel - self.vel.scale(drag);
        if self.spin != Fx::ZERO {
            let (dir, gs) = g.norm_len();
            let side = dir.perp().scale(self.spin.mul(MAGNUS).mul(gs).mul(DT));
            self.vel.x += side.x;
            self.vel.z += side.y;
            let decay = SPIN_DECAY.mul(DT);
            self.spin = self.spin - self.spin.mul(decay);
            if self.spin.abs() < Fx::milli(5) {
                self.spin = Fx::ZERO;
            }
        }

        let on_ground = self.pos.y <= BALL_R + Fx::milli(2) && self.vel.y <= Fx::ZERO;
        if on_ground && self.vel.y.abs() < SETTLE_VY {
            // Rolling: resistance + grass drag; no gravity work.
            self.vel.y = Fx::ZERO;
            self.pos.y = BALL_R;
            let (dir, gs) = self.vel.ground().norm_len();
            let (decel, drag) = if wet {
                (
                    ROLL_DECEL.mul(Fx::milli(650)),
                    GRASS_DRAG.mul(Fx::milli(700)),
                )
            } else {
                (ROLL_DECEL, GRASS_DRAG)
            };
            let lose = (decel + drag.mul(gs)).mul(DT);
            let ns = (gs - lose).max(Fx::ZERO);
            let v = dir.scale(ns);
            self.vel.x = v.x;
            self.vel.z = v.y;
            // A rolling ball barely curls.
            self.spin = self.spin.mul(Fx::milli(900));
        } else {
            self.vel.y -= GRAVITY.mul(DT);
        }

        // Near a goal, move in substeps of under 10 cm so a fast shot can't
        // skip through a post or the bar between two ticks.
        let near_goal = self.pos.x.abs() > HALF_LEN - Fx::int(3);
        let steps = if near_goal {
            1 + (self.vel.len().mul(DT) / Fx::milli(100))
                .floor_int()
                .clamp(0, 15)
        } else {
            1
        };
        let part = self.vel.scale(DT);
        let part = V3::new(part.x / steps, part.y / steps, part.z / steps);
        for _ in 0..steps {
            self.pos += part;
            if near_goal && (self.collide_frame(End::East) || self.collide_frame(End::West)) {
                ev.hit_frame = true;
                break;
            }
        }

        if self.pos.y < BALL_R {
            self.pos.y = BALL_R;
            if self.vel.y < Fx::ZERO {
                let vy = -self.vel.y;
                self.vel.y = if vy < SETTLE_VY {
                    Fx::ZERO
                } else {
                    vy.mul(RESTITUTION)
                };
                let grip = if wet { Fx::milli(930) } else { BOUNCE_GRIP };
                self.vel.x = self.vel.x.mul(grip);
                self.vel.z = self.vel.z.mul(grip);
                self.spin = self.spin.mul(Fx::milli(600));
                ev.bounced = vy > Fx::int(2);
            }
        }

        ev.in_net = self.net(End::East) || self.net(End::West);
        ev
    }

    /// Posts (vertical cylinders) and the bar (a horizontal one) at `end`.
    fn collide_frame(&mut self, end: End) -> bool {
        let gx = end.goal_x();
        if (self.pos.x - gx).abs() > POST_R + BALL_R + Fx::milli(50) {
            return false;
        }
        let reach = POST_R + BALL_R;
        let mut hit = false;
        // Posts: centers just outside the goal mouth.
        for side in [Fx::ONE, -Fx::ONE] {
            if self.pos.y > GOAL_H + POST_R + BALL_R {
                continue;
            }
            let c = V2::new(gx, (GOAL_HALF_W + POST_R).mul(side));
            let d = self.pos.ground() - c;
            if d.shorter_than(reach) {
                let n = if d == V2::ZERO {
                    V2::new(-end.sign(), Fx::ZERO)
                } else {
                    d.norm()
                };
                hit |= self.reflect(
                    V3::new(n.x, Fx::ZERO, n.y),
                    V3::from_ground(c, self.pos.y),
                    reach,
                );
            }
        }
        // Bar: along y (world z) at the top of the mouth.
        if self.pos.z.abs() < GOAL_HALF_W + POST_R {
            let c = V3::new(gx, GOAL_H + POST_R, self.pos.z);
            let d = V2::new(self.pos.x - c.x, self.pos.y - c.y);
            if d.shorter_than(reach) {
                let n = if d == V2::ZERO {
                    V2::new(Fx::ZERO, Fx::ONE)
                } else {
                    d.norm()
                };
                hit |= self.reflect(V3::new(n.x, n.y, Fx::ZERO), c, reach);
            }
        }
        hit
    }

    /// Push out along `n` to `reach` from `c` and bounce the normal speed.
    fn reflect(&mut self, n: V3, c: V3, reach: Fx) -> bool {
        let vn = self.vel.x.mul(n.x) + self.vel.y.mul(n.y) + self.vel.z.mul(n.z);
        // Out of contact along the normal, measured from the frame's axis.
        if n.y == Fx::ZERO {
            let p = V2::new(c.x, c.z) + V2::new(n.x, n.z).scale(reach + Fx::milli(1));
            self.pos.x = p.x;
            self.pos.z = p.y;
        } else {
            self.pos.x = c.x + n.x.mul(reach + Fx::milli(1));
            self.pos.y = c.y + n.y.mul(reach + Fx::milli(1));
        }
        if vn >= Fx::ZERO {
            return false;
        }
        let k = vn.mul(Fx::ONE + POST_RESTITUTION);
        self.vel = self.vel - n.scale(k);
        self.spin = Fx::ZERO;
        true
    }

    /// Inside the goal at `end`: the net soaks up the ball. Returns `true`
    /// while the ball is in the net.
    fn net(&mut self, end: End) -> bool {
        let s = end.sign();
        let depth = (self.pos.x - end.goal_x()).mul(s);
        if depth <= BALL_R || self.pos.z.abs() >= GOAL_HALF_W || self.pos.y >= GOAL_H {
            return false;
        }
        // Back of the net.
        if depth > GOAL_DEPTH - BALL_R {
            self.pos.x = end.goal_x() + (GOAL_DEPTH - BALL_R).mul(s);
            self.vel.x = -self.vel.x.mul(Fx::milli(150));
        }
        // Side netting and roof.
        let side = GOAL_HALF_W - BALL_R;
        if self.pos.z.abs() > side {
            self.pos.z = side.mul(self.pos.z.signum());
            self.vel.z = -self.vel.z.mul(Fx::milli(150));
        }
        if self.pos.y > GOAL_H - BALL_R {
            self.pos.y = GOAL_H - BALL_R;
            self.vel.y = -self.vel.y.mul(Fx::milli(150));
        }
        // The net's give: heavy damping.
        self.vel = self.vel.scale(Fx::milli(930));
        true
    }
}

/// Kick parameters → initial velocity. `dir` is a unit ground direction,
/// `speed` the launch speed (m/s), `loft` the vertical share (0 = along
/// the turf, 0.7 ≈ 45°).
pub fn launch(dir: V2, speed: Fx, loft: Fx) -> V3 {
    let up = loft.clamp(Fx::ZERO, Fx::milli(900));
    // Keep |v| = speed: ground share sqrt(1 - up²).
    let ground = (Fx::ONE - up.mul(up)).sqrt();
    let g = dir.scale(speed.mul(ground));
    V3::new(g.x, speed.mul(up), g.y)
}

/// Speed steps of [`ROLL_TABLE`] (0.5 m/s).
const ROLL_STEP: i32 = 1 << 15;

/// One tick of a rolling ball's speed (raw Q16.16): turf resistance,
/// grass drag and air drag, the forces [`Ball::step`] applies on a dry
/// pitch.
const fn roll_speed_step(v: i64) -> i64 {
    let a = ROLL_DECEL.0 as i64;
    let c = GRASS_DRAG.0 as i64;
    let k = AIR_DRAG.0 as i64;
    let dt = DT.0 as i64;
    let lose = ((a + ((c * v) >> 16) + ((((k * v) >> 16) * v) >> 16)) * dt) >> 16;
    let n = v - lose;
    if n < 0 { 0 } else { n }
}

/// Distance (m) a ball rolling at `i` × 0.5 m/s covers before it stops.
const fn roll_table() -> [Fx; 81] {
    let mut out = [Fx::ZERO; 81];
    let mut i = 1;
    while i < 81 {
        let mut v = (i as i64) * ROLL_STEP as i64;
        let mut x = 0i64;
        let mut guard = 0;
        while v > 0 && guard < 60 * 60 {
            v = roll_speed_step(v);
            x += (v * DT.0 as i64) >> 16;
            guard += 1;
        }
        out[i] = Fx(x as i32);
        i += 1;
    }
    out
}

/// See [`roll_table`].
pub const ROLL_TABLE: [Fx; 81] = roll_table();

/// Ground distance a ball rolling at `speed` covers before it stops on a
/// dry pitch.
pub fn roll_distance(speed: Fx) -> Fx {
    let s = speed.max(Fx::ZERO);
    let i = (s.0 / ROLL_STEP) as usize;
    if i >= 80 {
        return ROLL_TABLE[80];
    }
    let f = Fx((s.0 % ROLL_STEP) * 2);
    ROLL_TABLE[i].lerp(ROLL_TABLE[i + 1], f)
}

/// The speed a ground pass needs to arrive at `dist` meters still rolling
/// at about `arrive` m/s: the roll law depends on speed only, so the run
/// from `v` down to `arrive` is `D(v) - D(arrive)`.
pub fn pass_speed(dist: Fx, arrive: Fx) -> Fx {
    let want = dist + roll_distance(arrive);
    let mut i = 1;
    while i < 80 && ROLL_TABLE[i] < want {
        i += 1;
    }
    let (lo, hi) = (ROLL_TABLE[i - 1], ROLL_TABLE[i]);
    let f = if hi > lo {
        ((want - lo).div(hi - lo)).clamp(Fx::ZERO, Fx::ONE)
    } else {
        Fx::ZERO
    };
    Fx(((i as i32 - 1) * ROLL_STEP) + f.mul(Fx(ROLL_STEP)).0)
}

/// Where the ball (with its spin) will be, `ticks` ahead, if nobody touches
/// it. Used by keepers, receivers and the HUD's trajectory arc.
pub fn predict(ball: &Ball, ticks: u32, wet: bool) -> Ball {
    let mut b = *ball;
    for _ in 0..ticks {
        b.step(wet);
    }
    b
}

/// The ball center is wholly past the goal line between the posts and
/// under the bar: a goal at `end` (the defending side's end).
pub fn goal_scored(ball: &Ball) -> Option<End> {
    let end = pitch::over_goal_line(ball.pos.ground())?;
    pitch::inside_goal_frame(ball.pos.ground(), ball.pos.y).then_some(end)
}

/// Is `x` past the goal line at `end` (for the net test in the HUD)?
pub fn past_line(x: Fx, end: End) -> bool {
    (x - end.goal_x()).mul(end.sign()) > BALL_R
}

/// A ball crossing the plane of the goal at `end`: (ticks until it does,
/// where it crosses (y), and its height there), within `horizon` ticks.
pub fn crossing(ball: &Ball, end: End, horizon: u32, wet: bool) -> Option<(u32, Fx, Fx)> {
    let mut b = *ball;
    let gx = end.goal_x();
    let s = end.sign();
    if (b.vel.x).mul(s) <= Fx::ZERO {
        return None;
    }
    for t in 1..=horizon {
        b.step(wet);
        if (b.pos.x - gx).mul(s) >= Fx::ZERO {
            return Some((t, b.pos.z, b.pos.y));
        }
        if b.vel.ground().shorter_than(Fx::milli(300)) {
            return None;
        }
    }
    None
}

/// Half the pitch length, for callers that only import this module.
pub const LINE_X: Fx = HALF_LEN;

#[cfg(test)]
mod tests {
    use super::*;

    fn run(b: &mut Ball, ticks: u32) {
        for _ in 0..ticks {
            b.step(false);
        }
    }

    #[test]
    fn a_rolling_ball_slows_and_stops() {
        let mut b = Ball::at_rest(V2::ZERO);
        b.vel = V3::new(Fx::int(10), Fx::ZERO, Fx::ZERO);
        run(&mut b, 60);
        let after1 = b.vel.x;
        assert!(
            after1 < Fx::int(9) && after1 > Fx::int(7),
            "{}",
            after1.to_f32()
        );
        run(&mut b, 60 * 15);
        assert_eq!(b.vel, V3::ZERO);
        let d = b.pos.x.to_f32();
        // 10 m/s along the turf: a 25–40 m pass.
        assert!((25.0..40.0).contains(&d), "{d}");
        assert_eq!(b.pos.y, BALL_R);
    }

    #[test]
    fn a_wet_pitch_lets_the_ball_run_further() {
        let mut dry = Ball::at_rest(V2::ZERO);
        dry.vel = V3::new(Fx::int(12), Fx::ZERO, Fx::ZERO);
        let mut wet = dry;
        for _ in 0..600 {
            dry.step(false);
            wet.step(true);
        }
        assert!(wet.pos.x > dry.pos.x + Fx::int(5));
    }

    #[test]
    fn a_lofted_ball_flies_bounces_lower_then_rolls() {
        let mut b = Ball::at_rest(V2::ZERO);
        b.vel = launch(V2::new(Fx::ONE, Fx::ZERO), Fx::int(20), Fx::milli(600));
        let mut peak = Fx::ZERO;
        let mut bounces = [Fx::ZERO; 3];
        let mut nb = 0;
        let mut was_up = false;
        for _ in 0..60 * 8 {
            let ev = b.step(false);
            peak = peak.max(b.pos.y);
            if b.pos.y > Fx::int(1) {
                was_up = true;
            }
            if ev.bounced && nb < 3 {
                bounces[nb] = b.vel.y;
                nb += 1;
            }
        }
        assert!(was_up);
        assert!(peak > Fx::int(5) && peak < Fx::int(12), "{}", peak.to_f32());
        assert!(nb >= 2);
        assert!(bounces[1] < bounces[0], "each bounce lower");
        assert!(b.grounded());
        assert!(b.pos.x > Fx::int(25), "a long ball: {}", b.pos.x.to_f32());
    }

    #[test]
    fn side_spin_curls_the_ball() {
        let mut straight = Ball::at_rest(V2::ZERO);
        straight.vel = launch(V2::new(Fx::ONE, Fx::ZERO), Fx::int(24), Fx::milli(150));
        let mut curled = straight;
        curled.spin = Fx::milli(400);
        for _ in 0..60 {
            straight.step(false);
            curled.step(false);
        }
        assert_eq!(straight.pos.z, Fx::ZERO);
        // Positive spin bends toward +perp of +x, i.e. +z (world).
        assert!(curled.pos.z > Fx::int(1), "{}", curled.pos.z.to_f32());
        assert!(curled.spin < Fx::milli(400));
    }

    #[test]
    fn the_post_sends_the_ball_back() {
        // Straight at the near post of the east goal, at knee height.
        let mut b = Ball::at_rest(V2::new(Fx::int(40), GOAL_HALF_W + POST_R));
        b.pos.y = Fx::milli(500);
        b.vel = V3::new(Fx::int(25), Fx::ZERO, Fx::ZERO);
        let mut hit = false;
        for _ in 0..60 {
            hit |= b.step(false).hit_frame;
        }
        assert!(hit);
        assert!(b.vel.x < Fx::ZERO, "rebounds out: {}", b.vel.x.to_f32());
        assert_eq!(goal_scored(&b), None);
    }

    #[test]
    fn the_bar_stops_a_rising_shot() {
        // Close in, so gravity hasn't pulled it under the bar yet.
        let mut b = Ball::at_rest(V2::new(Fx::int(51), Fx::ZERO));
        b.pos.y = GOAL_H + POST_R;
        b.vel = V3::new(Fx::int(25), Fx::ZERO, Fx::ZERO);
        let mut hit = false;
        for _ in 0..30 {
            hit |= b.step(false).hit_frame;
        }
        assert!(hit);
        assert!(b.vel.x < Fx::ZERO);
    }

    #[test]
    fn a_shot_into_the_net_is_a_goal_and_stays_in() {
        let mut b = Ball::at_rest(V2::new(Fx::int(40), Fx::int(1)));
        b.vel = launch(V2::new(Fx::ONE, Fx::ZERO), Fx::int(28), Fx::milli(80));
        let mut scored = None;
        let mut in_net = false;
        for _ in 0..120 {
            in_net |= b.step(false).in_net;
            scored = scored.or(goal_scored(&b));
        }
        assert_eq!(scored, Some(End::East));
        assert!(in_net);
        assert!(b.pos.x < HALF_LEN + GOAL_DEPTH);
        assert!(b.pos.x > HALF_LEN);
    }

    #[test]
    fn pass_speed_reaches_the_target() {
        for d in [8, 15, 25, 35] {
            let v = pass_speed(Fx::int(d), Fx::int(3));
            // Roll it for real: it must pass the target still moving.
            let mut b = Ball::at_rest(V2::ZERO);
            b.vel = V3::new(v, Fx::ZERO, Fx::ZERO);
            while b.pos.x < Fx::int(d) && b.vel.x > Fx::ZERO {
                b.step(false);
            }
            let at = b.vel.x.to_f32();
            assert!(
                (2.0..4.0).contains(&at),
                "{d} m: speed {} arrives at {at}",
                v.to_f32()
            );
        }
    }

    #[test]
    fn crossing_predicts_where_a_shot_meets_the_line() {
        let mut b = Ball::at_rest(V2::new(Fx::int(36), Fx::ZERO));
        b.vel = launch(
            V2::new(Fx::ONE, Fx::milli(100)).norm(),
            Fx::int(26),
            Fx::milli(60),
        );
        let (t, y, h) = crossing(&b, End::East, 120, false).unwrap();
        let mut real = b;
        for _ in 0..t {
            real.step(false);
        }
        assert_eq!(real.pos.z, y);
        assert_eq!(real.pos.y, h);
        assert!(y > Fx::ZERO);
        assert_eq!(crossing(&b, End::West, 120, false), None);
    }
}
