//! Match scenarios: every restart, foul, card and set piece the sim knows,
//! staged on a real match and played through the engine, checking both the
//! referee's call and what each player involved is shown doing (the clip
//! the game picks, and that his pose fits the body: `render::fit`). Then
//! whole matches in different styles — rain, difficulty, tactics, a human
//! keeper — with their frames checked the same way.

extern crate std;
use std::string::String;
use std::vec::Vec;

use crate::render::anim::AnimState;
use crate::render::clips_gen::{ClipName, JOINTS};
use crate::render::{fit, pick_clip, pick_official_clip};
use crate::sim::engine::{Act, Event, EventKind, Match, N, Pad, Phase, SIDE, Setup};
use crate::sim::fx::{Fx, V2};
use crate::sim::pitch;
use crate::sim::rules::RestartKind;
use crate::sim::team::Role;

const DT: f32 = 1.0 / 60.0;

/// A gait shown for fewer ticks than this between two of the same is a
/// flicker (an eighth of a second: shorter than the crossfade).
const FLICKER: u32 = 8;

/// A match being staged and played tick by tick, with the clips the game
/// would show.
struct Stage {
    m: Match,
    anims: [AnimState; N + 3],
    prev_officials: [(f32, f32); 3],
    seen: u32,
    log: Vec<Event>,
    /// Every clip each body was shown, in order (players, then officials).
    shown: Vec<Vec<ClipName>>,
    /// When each body last changed clip, and the most changes it made in
    /// any one second (a flickering clip).
    changes: Vec<Vec<u32>>,
    /// Frames checked against the body's fit, and what broke.
    checked: u32,
    breaches: Vec<String>,
}

impl Stage {
    fn from_setup(st: &Setup) -> Stage {
        let m = Match::new(st);
        let prev_officials =
            core::array::from_fn(|k| (m.officials[k].x.to_f32(), m.officials[k].y.to_f32()));
        Stage {
            seen: m.event_seq,
            m,
            anims: [AnimState::new(ClipName::Idle); N + 3],
            prev_officials,
            log: Vec::new(),
            shown: (0..N + 3).map(|_| Vec::new()).collect(),
            changes: (0..N + 3).map(|_| Vec::new()).collect(),
            checked: 0,
            breaches: Vec::new(),
        }
    }

    /// FC Rouge v Azure United, AI v AI, played to open play.
    fn in_play(seed: u32) -> Stage {
        let (st, _, _) = crate::quick_setup(0, 1, seed, 180, [false, false]);
        let mut s = Stage::from_setup(&st);
        assert!(s.until(60 * 30, |m| m.phase == Phase::Play), "kick-off");
        s
    }

    fn step(&mut self, pads: [Pad; 2]) {
        self.m.step(pads);
        for (_, e) in self.m.events_since(self.seen) {
            self.log.push(e);
        }
        self.seen = self.m.event_seq;
        for i in 0..N {
            pick_clip(&self.m, i, DT, &mut self.anims[i]);
        }
        for k in 0..3 {
            pick_official_clip(
                &self.m,
                k,
                self.prev_officials[k],
                DT,
                &mut self.anims[N + k],
            );
            self.prev_officials[k] = (
                self.m.officials[k].x.to_f32(),
                self.m.officials[k].y.to_f32(),
            );
        }
        for (i, a) in self.anims.iter().enumerate() {
            if self.shown[i].last() != Some(&a.clip) {
                self.shown[i].push(a.clip);
                self.changes[i].push(self.m.tick);
            }
        }
    }

    fn run(&mut self, ticks: u32) {
        for _ in 0..ticks {
            self.step([Pad::default(); 2]);
        }
    }

    /// Step (no input) until `f` holds; false after `max` ticks.
    fn until(&mut self, max: u32, f: impl Fn(&Match) -> bool) -> bool {
        for _ in 0..max {
            if f(&self.m) {
                return true;
            }
            self.step([Pad::default(); 2]);
        }
        f(&self.m)
    }

    fn events(&self, kind: EventKind) -> Vec<Event> {
        self.log
            .iter()
            .copied()
            .filter(|e| e.kind == kind)
            .collect()
    }

    fn showed(&self, who: usize, clip: ClipName) -> bool {
        self.shown[who].contains(&clip)
    }

    /// Check every shown body's pose now against the fit rules (both the
    /// PC's body and the consoles').
    fn check_poses(&mut self, label: &str) {
        for (i, a) in self.anims.iter().enumerate() {
            if i < N && self.m.players[i].off {
                continue;
            }
            for full in [true, false] {
                let mask = if full {
                    crate::render::anim::mask::ALL
                } else {
                    crate::render::anim::mask::LIMBS
                };
                let pose = a.pose_masked(mask);
                for b in fit::pose_breaches(full, &pose, a.grounded(), label) {
                    self.breaches.push(format!("body {i} ({:?}): {b}", a.clip));
                }
                self.checked += 1;
            }
        }
    }

    /// Move the ball to `at` (pitch meters), loose, with `vel` (m/s).
    fn ball(&mut self, at: V2, vel: (i32, i32, i32)) {
        let mut b = crate::sim::ball::Ball::at_rest(at);
        b.vel = crate::sim::fx::V3::new(Fx::milli(vel.0), Fx::milli(vel.1), Fx::milli(vel.2));
        self.m.ball = b;
        self.m.owner = None;
    }

    /// Everyone but `keep` well away from `around` (no one interferes).
    fn clear_area(&mut self, around: V2, keep: &[usize]) {
        for i in 0..N {
            if keep.contains(&i) || self.m.players[i].role == Role::Gk {
                continue;
            }
            let p = self.m.players[i].pos;
            if (p - around).len() < Fx::int(16) {
                let y = if around.y > Fx::ZERO {
                    -Fx::int(25)
                } else {
                    Fx::int(25)
                };
                self.m.players[i].pos = V2::new(p.x, y);
                self.m.players[i].vel = V2::ZERO;
            }
        }
    }
}

fn v2(x: i32, y: i32) -> V2 {
    V2::new(Fx::int(x), Fx::int(y))
}

/// A side's first outfield player (slot 9: a forward).
fn forward(s: usize) -> usize {
    s * SIDE + 9
}

// ------------------------------------------------------------ restarts

#[test]
fn over_the_touchline_the_other_side_throws_it_in() {
    let mut s = Stage::in_play(3);
    let toucher = forward(0);
    s.m.last_touch = toucher as u8;
    s.ball(
        V2::new(Fx::int(10), pitch::HALF_WID - Fx::milli(300)),
        (0, 0, 6_000),
    );
    assert!(
        s.until(120, |m| m.phase != Phase::Play),
        "the ball went out"
    );
    let ev = s.events(EventKind::ThrowIn);
    assert_eq!(ev.len(), 1, "{:?}", s.log);
    assert_eq!(ev[0].side, 1, "the other side throws");
    assert_eq!(s.m.restart.kind, RestartKind::ThrowIn);
    assert!((s.m.restart.spot.y.abs() - pitch::HALF_WID).abs() < Fx::int(1));
    // The thrower winds up with the ball over his head, then throws.
    assert!(s.until(600, |m| matches!(m.phase, Phase::Ready { .. })));
    s.run(2);
    let taker = s.m.taker as usize;
    assert_eq!(s.anims[taker].clip, ClipName::Throw, "the thrower's clip");
    s.check_poses("throw-in wind-up");
    assert!(
        s.until(60 * 12, |m| m.phase == Phase::Play),
        "the throw was taken"
    );
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

#[test]
fn over_the_goal_line_a_goal_kick_or_a_corner() {
    for (last, kind, side) in [
        (0usize, RestartKind::Corner, 1u8),
        (1, RestartKind::GoalKick, 0),
    ] {
        let mut s = Stage::in_play(5);
        // Side 0 defends `ends[0]`: a ball over that goal line, wide of
        // the post. Last touched by a defender: a corner; by an attacker:
        // a goal kick.
        let end = s.m.ends[0];
        let toucher = if last == 0 { 3 } else { forward(1) };
        s.m.last_touch = toucher as u8;
        let x = end.goal_x() - end.sign().mul(Fx::milli(400));
        s.clear_area(V2::new(x, Fx::int(18)), &[]);
        s.ball(
            V2::new(x, Fx::int(18)),
            (end.sign().mul(Fx::int(6_000)).to_f32() as i32, 0, 0),
        );
        assert!(s.until(120, |m| m.phase != Phase::Play));
        assert_eq!(s.m.restart.kind, kind, "{:?}", s.log);
        assert_eq!(s.m.restart.team, side);
        let want = if kind == RestartKind::Corner {
            EventKind::Corner
        } else {
            EventKind::GoalKick
        };
        assert_eq!(s.events(want).len(), 1);
        assert!(s.until(60 * 12, |m| m.phase == Phase::Play), "taken");
    }
}

#[test]
fn a_goal_counts_its_scorer_celebrates_and_the_other_side_kicks_off() {
    let mut s = Stage::in_play(7);
    let scorer = forward(0);
    let goal = s.m.attacks(0);
    // A ball into the net, last touched by our forward.
    s.m.last_touch = scorer as u8;
    let from = goal.goal_center() - V2::new(goal.sign().mul(Fx::int(3)), Fx::ZERO);
    s.m.players[s.m.keeper_of(1).unwrap()].pos = goal.goal_center() + V2::new(Fx::ZERO, Fx::int(5));
    s.ball(
        from,
        ((goal.sign().mul(Fx::int(20_000))).to_f32() as i32, 500, 0),
    );
    assert!(s.until(120, |m| m.score[0] == 1), "{:?}", s.log);
    assert_eq!(s.events(EventKind::Goal { own_goal: false }).len(), 1);
    assert!(matches!(s.m.phase, Phase::Goal { side: 0, .. }));
    // His teammates celebrate (most of them, once they stop running).
    s.run(120);
    let celebrating = (0..SIDE)
        .filter(|&k| s.showed(k, ClipName::Celebrate))
        .count();
    assert!(celebrating >= 2, "{celebrating} of side 0 celebrated");
    // The scorer celebrates where he is; the others run to him first.
    assert!(
        s.showed(scorer, ClipName::Celebrate),
        "{:?}",
        s.shown[scorer]
    );
    s.check_poses("goal celebration");
    // Then the conceding side kicks off at the center.
    assert!(s.until(60 * 10, |m| !matches!(m.phase, Phase::Goal { .. })));
    assert_eq!(s.m.restart.kind, RestartKind::KickOff);
    assert_eq!(s.m.restart.team, 1);
    assert_eq!(s.m.restart.spot, V2::ZERO);
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

#[test]
fn half_time_swaps_ends_and_the_other_side_starts_the_second_half() {
    let mut s = Stage::in_play(9);
    let ends = s.m.ends;
    let first = s.m.restart.team;
    s.m.clock = s.m.half_ticks - 2;
    assert!(s.until(60 * 4, |m| matches!(m.phase, Phase::HalfTime { .. })));
    assert_eq!(s.events(EventKind::HalfTime).len(), 1);
    assert!(s.until(60 * 30, |m| m.half == 2 && m.phase == Phase::Play));
    assert_eq!(s.m.ends, [ends[1], ends[0]], "ends swapped");
    let kicked = s.events(EventKind::KickOff).last().copied().unwrap();
    assert_ne!(
        kicked.side, first,
        "the other side kicked off the second half"
    );
    // And full time ends it.
    s.m.clock = s.m.half_ticks - 2;
    assert!(s.until(60 * 4, |m| m.finished()));
    assert_eq!(s.events(EventKind::FullTime).len(), 1);
}

// ------------------------------------------------------- fouls, cards

/// A sliding tackle from behind on a carrier at `at` (side 0 has the
/// ball, side 1's player `i` slides): returns the stage after it.
fn slide_from_behind(at: V2, seed: u32, yellows: u8) -> (Stage, usize, usize) {
    let mut s = Stage::in_play(seed);
    s.m.humans = [false, true];
    let carrier = forward(0);
    let tackler = SIDE + 4;
    s.clear_area(at, &[carrier, tackler]);
    let fwd = s.m.attacks(0).sign();
    // The carrier stands with the ball a stride ahead: the slide's feet
    // meet his legs before the ball.
    s.m.players[carrier].pos = at;
    s.m.players[carrier].face = V2::new(fwd, Fx::ZERO);
    s.m.players[carrier].vel = V2::ZERO;
    s.m.players[carrier].act = Act::Free;
    s.ball(at + V2::new(fwd.mul(Fx::milli(1_500)), Fx::ZERO), (0, 0, 0));
    s.m.owner = Some(carrier as u8);
    s.m.players[tackler].pos = at - V2::new(fwd.mul(Fx::milli(1_200)), Fx::ZERO);
    s.m.players[tackler].face = V2::new(fwd, Fx::ZERO);
    s.m.players[tackler].yellow = yellows;
    s.m.players[tackler].act = Act::Free;
    s.m.active[1] = tackler as u8;
    let pad = Pad {
        slide: true,
        mv: V2::new(fwd, Fx::ZERO),
        ..Pad::default()
    };
    s.step([Pad::default(), pad]);
    let fouled = s.until(90, |m| m.phase != Phase::Play);
    assert!(fouled, "the slide stopped play: {:?}", s.log);
    (s, carrier, tackler)
}

#[test]
fn a_reckless_slide_from_behind_is_a_free_kick_and_a_yellow() {
    let (mut s, carrier, tackler) = slide_from_behind(v2(-5, 6), 21, 0);
    assert_eq!(s.events(EventKind::Foul).len(), 1, "{:?}", s.log);
    assert_eq!(
        s.events(EventKind::Yellow).len(),
        1,
        "from behind: a caution"
    );
    assert_eq!(s.m.players[tackler].yellow, 1);
    assert_eq!(s.m.restart.kind, RestartKind::FreeKick { direct: true });
    assert_eq!(s.m.restart.team, 0);
    // The fouled man goes down, the slider slid, the referee shows it.
    s.run(30);
    assert!(s.showed(carrier, ClipName::Down), "{:?}", s.shown[carrier]);
    assert!(s.showed(tackler, ClipName::Slide), "{:?}", s.shown[tackler]);
    assert!(s.showed(N, ClipName::RefCard), "{:?}", s.shown[N]);
    s.check_poses("foul");
    assert!(
        s.until(60 * 15, |m| m.phase == Phase::Play),
        "the free kick was taken"
    );
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

#[test]
fn a_second_yellow_sends_him_off_and_his_side_plays_on_with_ten() {
    let (s, _, tackler) = slide_from_behind(v2(-5, -6), 23, 1);
    assert_eq!(s.events(EventKind::Red).len(), 1, "{:?}", s.log);
    assert!(s.m.players[tackler].off);
    let on = (SIDE..N).filter(|&i| !s.m.players[i].off).count();
    assert_eq!(on, SIDE - 1);
}

#[test]
fn a_foul_in_the_box_is_a_penalty_taken_from_the_spot() {
    // Side 1 defends `ends[1]`; side 0 attacks it: the carrier in side 1's
    // box.
    let probe = Stage::in_play(25);
    let end = probe.m.ends[1];
    let at = V2::new(end.goal_x() - end.sign().mul(Fx::int(11)), Fx::int(4));
    drop(probe);
    let (mut s, _, _) = slide_from_behind(at, 25, 0);
    assert_eq!(s.events(EventKind::Penalty).len(), 1, "{:?}", s.log);
    assert_eq!(s.m.restart.kind, RestartKind::Penalty);
    assert_eq!(s.m.restart.team, 0);
    // The kick: the taker on the spot, the keeper on his line, everyone
    // else out of the box.
    assert!(s.until(60 * 10, |m| matches!(m.phase, Phase::Ready { .. })));
    let taker = s.m.taker as usize;
    assert!((s.m.players[taker].pos - end.pen_spot()).len() < Fx::int(2));
    let keeper = s.m.keeper_of(1).unwrap();
    assert!((s.m.players[keeper].pos.x - end.goal_x()).abs() < Fx::int(2));
    for i in 0..N {
        if i != taker && i != keeper && !s.m.players[i].off {
            assert!(
                !pitch::in_penalty_area(s.m.players[i].pos, end),
                "player {i} in the box at the kick"
            );
        }
    }
    // Taken (the kick winds up, then the ball leaves): a shot, maybe a
    // goal; the taker plays a shot clip (placed or full power).
    assert!(s.until(60 * 12, |m| m.phase == Phase::Play));
    s.run(40);
    assert!(
        s.showed(taker, ClipName::Shot) || s.showed(taker, ClipName::PowerShot),
        "{:?}",
        s.shown[taker]
    );
    assert_eq!(s.events(EventKind::Shot).len(), 1, "{:?}", s.log);
    s.check_poses("penalty");
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

#[test]
fn an_offside_pass_is_flagged_and_the_defenders_get_a_free_kick() {
    let mut s = Stage::in_play(27);
    s.m.humans = [true, false];
    let fwd = s.m.attacks(0).sign();
    let passer = 7;
    let runner = forward(0);
    // The defenders' line at +5 m (attack-x); the runner 6 m beyond it,
    // the passer in midfield with the ball.
    let line = 5;
    // The defenders on their line but out of the passing lane.
    for (k, i) in (SIDE..N).enumerate() {
        if s.m.players[i].role != Role::Gk {
            let y = if k % 2 == 0 {
                Fx::int(14 + k as i32)
            } else {
                -Fx::int(14 + k as i32)
            };
            s.m.players[i].pos =
                V2::new(fwd.mul(Fx::int(line)), y.clamp(-Fx::int(32), Fx::int(32)));
            s.m.players[i].vel = V2::ZERO;
        }
    }
    // His teammates out of the way too.
    for i in 0..SIDE {
        if i != passer && i != runner && s.m.players[i].role != Role::Gk {
            let p = s.m.players[i].pos;
            s.m.players[i].pos = V2::new(-fwd.mul(Fx::int(20)), p.y);
        }
    }
    s.m.players[runner].pos = V2::new(fwd.mul(Fx::int(line + 6)), Fx::ZERO);
    s.m.players[passer].pos = V2::new(-fwd.mul(Fx::int(8)), Fx::ZERO);
    s.m.players[passer].face = V2::new(fwd, Fx::ZERO);
    s.m.players[passer].act = Act::Free;
    s.ball(
        s.m.players[passer].pos + V2::new(fwd.mul(Fx::milli(400)), Fx::ZERO),
        (0, 0, 0),
    );
    s.m.owner = Some(passer as u8);
    s.m.active[0] = passer as u8;
    s.m.owned_since = 0;
    let pad = Pad {
        pass: Some(Fx::milli(400)),
        mv: V2::new(fwd, Fx::ZERO),
        ..Pad::default()
    };
    s.step([pad, Pad::default()]);
    assert!(s.until(60 * 4, |m| m.phase != Phase::Play), "{:?}", s.log);
    assert_eq!(s.events(EventKind::Offside).len(), 1, "{:?}", s.log);
    assert_eq!(s.m.restart.kind, RestartKind::FreeKick { direct: false });
    assert_eq!(s.m.restart.team, 1);
    // A linesman's flag is up, and he shows it.
    assert!(s.m.flag.1 > 0);
    s.run(20);
    let lm = N + s.m.flag.0 as usize;
    assert!(s.showed(lm, ClipName::FlagUp), "{:?}", s.shown[lm]);
    s.check_poses("offside flag");
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

#[test]
fn a_substitution_waits_for_a_dead_ball_and_stops_at_three() {
    let mut s = Stage::in_play(29);
    s.m.humans = [true, false];
    for _ in 0..5 {
        s.step([
            Pad {
                sub: true,
                ..Pad::default()
            },
            Pad::default(),
        ]);
        // A dead ball: out for a throw.
        s.m.last_touch = forward(1) as u8;
        let spot = V2::new(Fx::ZERO, pitch::HALF_WID - Fx::milli(200));
        s.clear_area(spot, &[]);
        s.ball(spot, (0, 0, 5_000));
        assert!(s.until(200, |m| m.phase != Phase::Play));
        assert!(s.until(60 * 15, |m| m.phase == Phase::Play));
    }
    let subs = s
        .log
        .iter()
        .filter(|e| e.kind == EventKind::Substitution && e.side == 0)
        .count();
    assert_eq!(subs, 3, "three substitutions, no more: {:?}", s.log);
}

#[test]
fn the_keeper_catches_a_soft_shot_holds_it_and_plays_it_out() {
    let mut s = Stage::in_play(31);
    let end = s.m.ends[1];
    let keeper = s.m.keeper_of(1).unwrap();
    s.m.players[keeper].pos = V2::new(end.goal_x() - end.sign().mul(Fx::int(2)), Fx::ZERO);
    let from = V2::new(end.goal_x() - end.sign().mul(Fx::int(14)), Fx::ZERO);
    s.clear_area(from, &[keeper]);
    s.m.last_touch = forward(0) as u8;
    s.ball(
        from,
        ((end.sign().mul(Fx::int(9_000))).to_f32() as i32, 300, 0),
    );
    assert!(
        s.until(120, |m| matches!(m.players[keeper].act, Act::Hold { .. })),
        "he held it: {:?}",
        s.m.players[keeper].act
    );
    s.run(4);
    assert_eq!(s.anims[keeper].clip, ClipName::GkHold);
    s.check_poses("keeper holding");
    // He plays it out (a throw or a punt) within his time.
    assert!(
        s.until(60 * 4, |m| m.owner != Some(keeper as u8)),
        "distributed"
    );
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

#[test]
fn the_keeper_dives_at_a_shot_into_the_corner() {
    let mut s = Stage::in_play(33);
    let end = s.m.ends[1];
    let keeper = s.m.keeper_of(1).unwrap();
    s.m.players[keeper].pos = V2::new(end.goal_x() - end.sign().mul(Fx::milli(800)), Fx::ZERO);
    let from = V2::new(end.goal_x() - end.sign().mul(Fx::int(11)), Fx::int(-1));
    s.clear_area(from, &[keeper]);
    s.m.last_touch = forward(0) as u8;
    s.m.shot_tick = s.m.tick;
    // Hard and low toward the far post: too far to walk to.
    let to = V2::new(end.goal_x(), Fx::milli(3_400));
    let dir = (to - from).norm().scale(Fx::int(30));
    s.ball(
        from,
        (
            dir.x.to_f32() as i32 * 1000,
            400,
            dir.y.to_f32() as i32 * 1000,
        ),
    );
    s.run(70);
    assert!(s.showed(keeper, ClipName::GkDive), "{:?}", s.shown[keeper]);
    s.check_poses("after a dive");
    s.run(60);
    s.check_poses("getting up");
    assert!(s.breaches.is_empty(), "{:?}", s.breaches);
}

// ------------------------------------------------------- whole matches

/// A human side played by a simple bot: run at the ball, pass or shoot
/// with it, take restarts; as the keeper (when he has him), dive at a
/// ball coming close.
fn bot(m: &Match, s: usize, tick: u32) -> Pad {
    let a = m.active[s] as usize;
    let me = m.players[a].pos;
    let b = m.ball.pos.ground();
    let (to_ball, d) = (b - me).norm_len();
    let mut pad = Pad {
        mv: to_ball,
        sprint: d > Fx::int(8),
        ..Pad::default()
    };
    let taker = matches!(m.phase, Phase::Ready { .. })
        && m.restart.team as usize == s
        && m.taker as usize == a;
    if m.owner == Some(a as u8) || taker {
        let goal = m.attacks(s).goal_center();
        let (to_goal, gd) = (goal - me).norm_len();
        pad.mv = to_goal;
        if tick.is_multiple_of(25) {
            if gd < Fx::int(22) {
                pad.shoot = Some(Fx::milli(700));
            } else {
                pad.pass = Some(Fx::milli(400));
            }
        }
    } else if m.players[a].role == Role::Gk {
        if d < Fx::int(5) && m.ball.vel.len() > Fx::int(5) && m.owner.is_none() {
            pad.tackle = true;
        }
    } else if d < Fx::milli(1_500) && tick.is_multiple_of(20) {
        pad.tackle = true;
    }
    pad
}

/// One style of match: the setup's tweak and a name.
type Style = (&'static str, fn(&mut Setup));

/// Whole matches in different styles. Each plays to full time with the
/// score equal to the goals logged and every player in the stadium, and
/// a sample of its frames (every body, every four seconds) fits the body.
#[test]
fn every_style_of_match_plays_out_and_every_frame_fits() {
    let styles: [Style; 7] = [
        ("dry, normal", |_| {}),
        ("rain", |st| st.wet = true),
        ("pressing, high lines", |st| {
            for s in &mut st.sides {
                s.tactics.press = 95;
                s.tactics.line = 90;
                s.tactics.aggression = 90;
            }
        }),
        ("deep blocks, long balls", |st| {
            for s in &mut st.sides {
                s.tactics.press = 10;
                s.tactics.line = 10;
                s.tactics.direct = 95;
            }
        }),
        ("hard AI v a human (bot)", |st| {
            st.humans = [true, false];
            st.difficulty = 2;
        }),
        ("easy AI v a human (bot)", |st| {
            st.humans = [true, false];
            st.difficulty = 0;
        }),
        ("two humans, keepers theirs", |st| {
            st.humans = [true, true];
            st.keeper_ctl = [true, true];
        }),
    ];
    let mut report = Vec::new();
    // Shots across every style: placed ones and full-power strikes.
    let (mut placed, mut power) = (0, 0);
    for (name, tweak) in styles {
        let (mut st, _, _) = crate::quick_setup(2, 5, 41, 90, [false, false]);
        tweak(&mut st);
        let mut s = Stage::from_setup(&st);
        let mut t = 0u32;
        while !s.m.finished() && t < st.half_ticks * 12 {
            let pads = core::array::from_fn(|side| {
                if st.humans[side] {
                    bot(&s.m, side, t)
                } else {
                    Pad::default()
                }
            });
            s.step(pads);
            t += 1;
            // Every body, every four seconds of play.
            if t.is_multiple_of(240) {
                s.check_poses(name);
            }
        }
        assert!(s.m.finished(), "{name}: never finished");
        for side in 0..2 {
            let goals = s
                .log
                .iter()
                .filter(|e| matches!(e.kind, EventKind::Goal { .. }) && e.side as usize == side)
                .count();
            assert_eq!(s.m.score[side] as usize, goals, "{name}");
        }
        for p in &s.m.players {
            assert!(
                p.pos.x.abs() < pitch::HALF_LEN + pitch::RUNOFF + Fx::ONE,
                "{name}"
            );
            assert!(
                p.pos.y.abs() < pitch::HALF_WID + pitch::RUNOFF + Fx::int(7),
                "{name}"
            );
        }
        let clips: usize = s.shown.iter().map(|v| v.len()).sum();
        for v in &s.shown {
            placed += v.iter().filter(|&&c| c == ClipName::Shot).count();
            power += v.iter().filter(|&&c| c == ClipName::PowerShot).count();
        }
        // No clip flickers: a run cycle never flips to another and back
        // within an eighth of a second (actions — a shot, a tackle — may
        // follow each other fast), and under about one change a second on
        // average.
        // (A dribble is the ball at his feet: winning and losing it fast
        // is play, not a flicker.)
        let gait = |c: ClipName| {
            matches!(
                c,
                ClipName::Idle
                    | ClipName::Walk
                    | ClipName::Jog
                    | ClipName::Sprint
                    | ClipName::GkReady
            )
        };
        let mut flips = 0;
        let mut example = String::new();
        for (b, (ticks, clips)) in s.changes.iter().zip(&s.shown).enumerate() {
            for k in 2..clips.len() {
                let (a, m, c) = (clips[k - 2], clips[k - 1], clips[k]);
                if gait(a) && gait(m) && a == c && ticks[k] - ticks[k - 1] < FLICKER {
                    flips += 1;
                    if example.len() < 400 {
                        example += &format!(
                            "body {b} t{}: {a:?} -> {m:?} ({} ticks) -> {c:?}; ",
                            ticks[k - 1],
                            ticks[k] - ticks[k - 1]
                        );
                    }
                }
            }
        }
        let secs = s.m.tick as f32 / 60.0;
        let rate = clips as f32 / (N + 3) as f32 / secs;
        assert!(flips <= 4, "{name}: {flips} gait flips ({example})");
        assert!(
            rate <= 1.2,
            "{name}: {rate:.2} clip changes per body per second"
        );
        report.push(format!(
            "{name}: {}-{}, {} events, {} clip changes ({rate:.2} per body per second, {flips} gait flips), {} poses checked",
            s.m.score[0],
            s.m.score[1],
            s.log.len(),
            clips,
            s.checked
        ));
        assert!(
            s.breaches.is_empty(),
            "{name}: {:?}",
            &s.breaches[..s.breaches.len().min(5)]
        );
    }
    for line in report {
        std::println!("{line}");
    }
    std::println!("shots shown: {placed} placed, {power} full power");
    assert!(
        placed > 0 && power > 0,
        "{placed} placed, {power} power shots"
    );
    let _ = JOINTS;
}
