//! The referee's decisions as pure functions (Laws of the Game 9–17,
//! simplified for an arcade match): out of play and the restart it gives,
//! offside position, fouls and their sanction. The match engine calls
//! these; the unit tests below are the rules matrix.

use super::fx::{Fx, V2};
use super::pitch::{self, End, HALF_LEN, HALF_WID};

/// How play restarts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum RestartKind {
    KickOff,
    ThrowIn,
    GoalKick,
    Corner,
    /// `direct`: a goal may be scored straight from it.
    FreeKick {
        direct: bool,
    },
    Penalty,
}

impl RestartKind {
    /// Law 11: no offside offence straight from these.
    pub fn exempt_from_offside(self) -> bool {
        matches!(
            self,
            RestartKind::ThrowIn | RestartKind::GoalKick | RestartKind::Corner
        )
    }

    /// How far opponents must stand from the ball (meters).
    pub fn distance(self) -> Fx {
        match self {
            RestartKind::ThrowIn => Fx::int(2),
            _ => pitch::CIRCLE_R,
        }
    }
}

/// A restart: what, for whom, from where.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Restart {
    pub kind: RestartKind,
    /// Side index (0 or 1) taking it.
    pub team: u8,
    pub spot: V2,
}

/// The ball left the field at `at` (its center, ground), last touched by
/// side `last_team`; `ends[s]` is the end side `s` defends. `None` while
/// the ball is in play. Goals are decided separately (a ball in the goal
/// is not "out" for a restart).
pub fn out_of_play(at: V2, height: Fx, last_team: u8, ends: [End; 2]) -> Option<Restart> {
    let other = 1 - last_team;
    if pitch::over_touchline(at) {
        // Throw-in where it crossed, for the side that didn't touch it.
        let y = HALF_WID.mul(at.y.signum());
        let x = at.x.clamp(-HALF_LEN + Fx::int(1), HALF_LEN - Fx::int(1));
        return Some(Restart {
            kind: RestartKind::ThrowIn,
            team: other,
            spot: V2::new(x, y),
        });
    }
    let end = pitch::over_goal_line(at)?;
    if pitch::inside_goal_frame(at, height) {
        return None; // a goal, not a restart
    }
    // Which side defends this end?
    let defender = if ends[0] == end { 0 } else { 1 };
    if last_team == defender {
        // Corner to the attackers, on the side it went out.
        let y = HALF_WID.mul(at.y.signum()) - Fx::milli(500).mul(at.y.signum());
        let x = end.goal_x() - Fx::milli(500).mul(end.sign());
        Some(Restart {
            kind: RestartKind::Corner,
            team: 1 - defender,
            spot: V2::new(x, y),
        })
    } else {
        // Goal kick from the goal area's edge, on the side it went out.
        let x = end.goal_x() - pitch::GOAL_AREA_DEPTH.mul(end.sign());
        let y = Fx::int(5).mul(at.y.signum());
        Some(Restart {
            kind: RestartKind::GoalKick,
            team: defender,
            spot: V2::new(x, y),
        })
    }
}

/// Position along the attack direction of a side defending `own`: larger
/// = nearer the opponents' goal line.
#[inline]
pub fn attack_x(p: V2, own: End) -> Fx {
    -p.x.mul(own.sign())
}

/// The second-last opponent's attack-x (the keeper usually being the
/// last): the offside line. `opps` are the defenders' attack-x values as
/// seen by the attacking side. With fewer than two opponents, the goal
/// line.
pub fn second_last(opps: &[Fx]) -> Fx {
    let mut first = -Fx::MAX;
    let mut second = -Fx::MAX;
    for &x in opps {
        if x > first {
            second = first;
            first = x;
        } else if x > second {
            second = x;
        }
    }
    if second == -Fx::MAX { HALF_LEN } else { second }
}

/// Law 11 offside **position**, all in the attacking side's attack-x:
/// in the opponents' half, and nearer their goal line than both the ball
/// and the second-last opponent. Level is onside.
pub fn offside_position(player: Fx, ball: Fx, second_last_opp: Fx) -> bool {
    player > Fx::ZERO && player > ball && player > second_last_opp
}

/// How a foul is punished.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Card {
    None,
    Yellow,
    Red,
}

/// What kind of challenge it was.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Challenge {
    /// A sliding tackle (feet off the ground).
    pub slide: bool,
    /// Into the back of the opponent (the tackler's direction against the
    /// victim's facing).
    pub from_behind: bool,
    /// Speed of the tackler at contact, m/s.
    pub speed: Fx,
    /// The victim was through on goal: no other defender but the keeper
    /// between him and the goal, within shooting reach (Law 12: denying
    /// an obvious goal-scoring opportunity).
    pub dogso: bool,
}

/// The sanction for a foul (careless: none; reckless: yellow; excessive
/// force or DOGSO: red). A penalty-area DOGSO with a genuine attempt at the
/// ball is cautioned only (the "triple punishment" rule).
pub fn sanction(c: Challenge, in_own_box: bool) -> Card {
    // Reckless: a slide from behind or at speed, or a standing challenge
    // that charges into the man's back. A standing tackle that just
    // clips him is careless.
    let reckless = if c.slide {
        c.from_behind || c.speed > Fx::milli(7_200)
    } else {
        c.from_behind && c.speed > Fx::int(5)
    };
    let excessive = c.slide && c.from_behind && c.speed > Fx::milli(7_600);
    if excessive {
        return Card::Red;
    }
    if c.dogso {
        return if in_own_box && !c.from_behind {
            Card::Yellow
        } else {
            Card::Red
        };
    }
    if reckless { Card::Yellow } else { Card::None }
}

/// Where a foul at `at` against side `fouled` is taken from: a penalty
/// when the offending side commits it in its own penalty area.
pub fn foul_restart(at: V2, fouled: u8, offender_end: End) -> Restart {
    if pitch::in_penalty_area(at, offender_end) {
        Restart {
            kind: RestartKind::Penalty,
            team: fouled,
            spot: offender_end.pen_spot(),
        }
    } else {
        Restart {
            kind: RestartKind::FreeKick { direct: true },
            team: fouled,
            spot: pitch::clamp_to_field(at),
        }
    }
}

/// A second yellow is a red.
pub fn accumulate(yellows: u8, card: Card) -> (u8, bool) {
    match card {
        Card::None => (yellows, false),
        Card::Yellow => (yellows + 1, yellows + 1 >= 2),
        Card::Red => (yellows, true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn p(x: i32, y: i32) -> V2 {
        V2::new(Fx::int(x), Fx::int(y))
    }

    const ENDS: [End; 2] = [End::West, End::East];

    #[test]
    fn touchline_gives_a_throw_in_to_the_other_side() {
        let r = out_of_play(p(10, 35), Fx::milli(110), 0, ENDS).unwrap();
        assert_eq!(r.kind, RestartKind::ThrowIn);
        assert_eq!(r.team, 1);
        assert_eq!(r.spot, V2::new(Fx::int(10), HALF_WID));
        let r = out_of_play(p(-60, -35), Fx::milli(110), 1, ENDS).unwrap();
        assert_eq!(r.team, 0);
        assert!(r.spot.x > -HALF_LEN, "kept on the pitch");
        // Still on the line: in play.
        assert_eq!(out_of_play(p(10, 34), Fx::milli(110), 0, ENDS), None);
    }

    #[test]
    fn goal_line_restart_depends_on_the_last_touch() {
        // Side 1 defends the east end. Side 0 shoots wide: goal kick.
        let r = out_of_play(p(53, 10), Fx::ONE, 0, ENDS).unwrap();
        assert_eq!(r.kind, RestartKind::GoalKick);
        assert_eq!(r.team, 1);
        assert!(r.spot.x < HALF_LEN && r.spot.y > Fx::ZERO);
        // A defender put it out: corner, on that side.
        let r = out_of_play(p(53, -10), Fx::ONE, 1, ENDS).unwrap();
        assert_eq!(r.kind, RestartKind::Corner);
        assert_eq!(r.team, 0);
        assert!(r.spot.y < Fx::ZERO && r.spot.x > Fx::int(51));
        // In the goal: not a restart (the goal check owns it).
        assert_eq!(out_of_play(p(53, 1), Fx::ONE, 0, ENDS), None);
        // Over the bar is out.
        assert!(out_of_play(p(53, 1), Fx::int(3), 0, ENDS).is_some());
    }

    #[test]
    fn ends_swap_at_half_time() {
        let swapped = [End::East, End::West];
        let r = out_of_play(p(53, 10), Fx::ONE, 0, swapped).unwrap();
        // Side 0 now defends east: its own touch is a corner for side 1.
        assert_eq!((r.kind, r.team), (RestartKind::Corner, 1));
    }

    #[test]
    fn offside_position_matrix() {
        let line = second_last(&[Fx::int(50), Fx::int(30), Fx::int(20)]);
        assert_eq!(
            line,
            Fx::int(30),
            "the keeper is last, the 30 m man second-last"
        );
        // Beyond the line and the ball, in their half: offside.
        assert!(offside_position(Fx::int(32), Fx::int(10), line));
        // Level: onside.
        assert!(!offside_position(Fx::int(30), Fx::int(10), line));
        // Behind the ball: onside.
        assert!(!offside_position(Fx::int(32), Fx::int(33), line));
        // Own half: never offside.
        let deep = second_last(&[Fx::int(-20), Fx::int(-30)]);
        assert!(!offside_position(Fx::int(-5), Fx::int(-40), deep));
        // The second-last may be an outfield player even if the keeper
        // came out (keeper at 20, last defender at 35).
        let line = second_last(&[Fx::int(20), Fx::int(35), Fx::int(10)]);
        assert_eq!(line, Fx::int(20));
        assert!(offside_position(Fx::int(25), Fx::int(5), line));
        // Exempt restarts.
        assert!(RestartKind::ThrowIn.exempt_from_offside());
        assert!(RestartKind::GoalKick.exempt_from_offside());
        assert!(RestartKind::Corner.exempt_from_offside());
        assert!(!RestartKind::FreeKick { direct: true }.exempt_from_offside());
        assert!(!RestartKind::KickOff.exempt_from_offside());
        // A lone defender: the line is the goal line.
        assert_eq!(second_last(&[Fx::int(40)]), HALF_LEN);
    }

    #[test]
    fn sanctions_follow_law_12() {
        let base = Challenge {
            slide: false,
            from_behind: false,
            speed: Fx::int(4),
            dogso: false,
        };
        assert_eq!(sanction(base, false), Card::None, "careless");
        assert_eq!(
            sanction(
                Challenge {
                    slide: true,
                    speed: Fx::int(6),
                    ..base
                },
                false
            ),
            Card::None,
            "a slide that arrives slowly is careless"
        );
        assert_eq!(
            sanction(
                Challenge {
                    from_behind: true,
                    ..base
                },
                false
            ),
            Card::None,
            "a standing tackle from behind at a walk: careless"
        );
        assert_eq!(
            sanction(
                Challenge {
                    from_behind: true,
                    speed: Fx::int(6),
                    ..base
                },
                false
            ),
            Card::Yellow,
            "reckless: charging into his back"
        );
        assert_eq!(
            sanction(
                Challenge {
                    slide: true,
                    from_behind: true,
                    speed: Fx::int(3),
                    ..base
                },
                false
            ),
            Card::Yellow,
            "reckless: a slide from behind"
        );
        assert_eq!(
            sanction(
                Challenge {
                    slide: true,
                    speed: Fx::milli(7_500),
                    ..base
                },
                false
            ),
            Card::Yellow,
            "reckless: fast slide"
        );
        assert_eq!(
            sanction(
                Challenge {
                    slide: true,
                    from_behind: true,
                    speed: Fx::int(8),
                    ..base
                },
                false
            ),
            Card::Red,
            "excessive force"
        );
        assert_eq!(
            sanction(
                Challenge {
                    dogso: true,
                    ..base
                },
                false
            ),
            Card::Red
        );
        assert_eq!(
            sanction(
                Challenge {
                    dogso: true,
                    ..base
                },
                true
            ),
            Card::Yellow,
            "DOGSO in the box with a try for the ball: caution"
        );
    }

    #[test]
    fn fouls_in_the_box_are_penalties() {
        // Side 1 defends east; a foul at (45, 3) on side 0.
        let r = foul_restart(p(45, 3), 0, End::East);
        assert_eq!(r.kind, RestartKind::Penalty);
        assert_eq!(r.spot, End::East.pen_spot());
        let r = foul_restart(p(30, 3), 0, End::East);
        assert_eq!(r.kind, RestartKind::FreeKick { direct: true });
        assert_eq!(r.spot, p(30, 3));
        // In the attacker's own box: a free kick, not a penalty.
        let r = foul_restart(p(-45, 3), 0, End::East);
        assert_eq!(r.kind, RestartKind::FreeKick { direct: true });
    }

    #[test]
    fn two_yellows_make_a_red() {
        assert_eq!(accumulate(0, Card::Yellow), (1, false));
        assert_eq!(accumulate(1, Card::Yellow), (2, true));
        assert_eq!(accumulate(0, Card::Red), (0, true));
        assert_eq!(accumulate(1, Card::None), (1, false));
    }
}
