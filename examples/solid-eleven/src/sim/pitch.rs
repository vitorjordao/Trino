//! The field of play, in meters (Laws of the Game, law 1, at the common
//! international size). The origin is the center spot; `x` runs along the
//! length (goal lines at ±[`HALF_LEN`]), `y` (the world's z) across it
//! (touchlines at ±[`HALF_WID`]).

use super::fx::{Fx, V2};

pub const HALF_LEN: Fx = Fx::milli(52_500);
pub const HALF_WID: Fx = Fx::milli(34_000);
/// Inside of the posts, from the center of the goal.
pub const GOAL_HALF_W: Fx = Fx::milli(3_660);
/// Underside of the crossbar.
pub const GOAL_H: Fx = Fx::milli(2_440);
/// Radius of the posts and the bar.
pub const POST_R: Fx = Fx::milli(60);
/// How deep the net runs behind the goal line.
pub const GOAL_DEPTH: Fx = Fx::milli(2_000);
pub const PEN_AREA_DEPTH: Fx = Fx::milli(16_500);
pub const PEN_AREA_HALF_W: Fx = Fx::milli(20_160);
pub const GOAL_AREA_DEPTH: Fx = Fx::milli(5_500);
pub const GOAL_AREA_HALF_W: Fx = Fx::milli(9_160);
/// Penalty mark, from the goal line.
pub const PEN_SPOT: Fx = Fx::milli(11_000);
pub const CIRCLE_R: Fx = Fx::milli(9_150);
pub const BALL_R: Fx = Fx::milli(110);
/// Run-off around the lines where play can still be (players, not the ball).
pub const RUNOFF: Fx = Fx::milli(4_000);

/// The goal a side defends: -1 = the goal at x = -52.5, +1 = x = +52.5.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum End {
    West = -1,
    East = 1,
}

impl End {
    #[inline]
    pub fn sign(self) -> Fx {
        match self {
            End::West => Fx(-(1 << 16)),
            End::East => Fx::ONE,
        }
    }

    #[inline]
    pub fn other(self) -> End {
        match self {
            End::West => End::East,
            End::East => End::West,
        }
    }

    /// The goal line's x.
    #[inline]
    pub fn goal_x(self) -> Fx {
        HALF_LEN.mul(self.sign())
    }

    /// Center of this end's goal mouth.
    #[inline]
    pub fn goal_center(self) -> V2 {
        V2::new(self.goal_x(), Fx::ZERO)
    }

    /// This end's penalty mark.
    pub fn pen_spot(self) -> V2 {
        V2::new((HALF_LEN - PEN_SPOT).mul(self.sign()), Fx::ZERO)
    }
}

/// Inside the penalty area at `end` (lines are part of the area).
pub fn in_penalty_area(p: V2, end: End) -> bool {
    let depth = (end.goal_x() - p.x).mul(end.sign());
    depth >= Fx::ZERO && depth <= PEN_AREA_DEPTH && p.y.abs() <= PEN_AREA_HALF_W
}

/// Inside the goal area at `end`.
pub fn in_goal_area(p: V2, end: End) -> bool {
    let depth = (end.goal_x() - p.x).mul(end.sign());
    depth >= Fx::ZERO && depth <= GOAL_AREA_DEPTH && p.y.abs() <= GOAL_AREA_HALF_W
}

/// The ball (center `p`, radius [`BALL_R`]) is wholly over a touchline.
pub fn over_touchline(p: V2) -> bool {
    p.y.abs() > HALF_WID + BALL_R
}

/// The ball is wholly over a goal line; returns which.
pub fn over_goal_line(p: V2) -> Option<End> {
    if p.x > HALF_LEN + BALL_R {
        Some(End::East)
    } else if p.x < -(HALF_LEN + BALL_R) {
        Some(End::West)
    } else {
        None
    }
}

/// Between the posts and under the bar (for a ball wholly over the line).
pub fn inside_goal_frame(p: V2, height: Fx) -> bool {
    p.y.abs() < GOAL_HALF_W && height < GOAL_H
}

/// Clamp a point to the playing surface plus the run-off.
pub fn clamp_to_surround(p: V2) -> V2 {
    V2::new(
        p.x.clamp(-(HALF_LEN + RUNOFF), HALF_LEN + RUNOFF),
        p.y.clamp(-(HALF_WID + RUNOFF), HALF_WID + RUNOFF),
    )
}

/// Clamp a point to the field of play (inside the lines).
pub fn clamp_to_field(p: V2) -> V2 {
    V2::new(
        p.x.clamp(-HALF_LEN, HALF_LEN),
        p.y.clamp(-HALF_WID, HALF_WID),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i32, y: i32) -> V2 {
        V2::milli(x, y)
    }

    #[test]
    fn areas_follow_the_laws() {
        assert!(in_penalty_area(p(40_000, 0), End::East));
        assert!(
            in_penalty_area(p(36_000, 20_160), End::East),
            "the line is in"
        );
        assert!(!in_penalty_area(p(35_900, 0), End::East));
        assert!(!in_penalty_area(p(40_000, 0), End::West));
        assert!(in_penalty_area(p(-50_000, -10_000), End::West));
        assert!(in_goal_area(p(-48_000, 9_000), End::West));
        assert!(!in_goal_area(p(-46_000, 0), End::West));
        assert_eq!(End::East.pen_spot(), p(41_500, 0));
        assert_eq!(End::West.pen_spot(), p(-41_500, 0));
    }

    #[test]
    fn the_whole_ball_must_cross_a_line() {
        // On the line, or still touching it: in play.
        assert!(!over_touchline(p(0, 34_000)));
        assert!(!over_touchline(p(0, 34_110)));
        assert!(over_touchline(p(0, 34_111)));
        assert_eq!(over_goal_line(p(52_600, 0)), None);
        assert_eq!(over_goal_line(p(52_611, 0)), Some(End::East));
        assert_eq!(over_goal_line(p(-52_611, 0)), Some(End::West));
        assert!(inside_goal_frame(p(52_700, 3_000), Fx::int(2)));
        assert!(!inside_goal_frame(p(52_700, 3_700), Fx::int(1)));
        assert!(!inside_goal_frame(p(52_700, 0), Fx::milli(2_500)));
    }
}
