//! Tactical AI. Each side keeps a **shape** (its formation slid with the
//! ball, compact without it, stretched with it, the back line held flat
//! for the offside trap), and each player has a job:
//!
//! - **Ball carrier:** a utility choice between shooting, passing (safe
//!   lanes, progress, receiver space, through balls behind the line),
//!   dribbling into space and clearing under pressure.
//! - **In possession, off the ball:** support near the shape slot, find a
//!   clear passing lane, forwards hold the offside line and run behind
//!   it, full backs overlap.
//! - **Out of possession:** the nearest presses the carrier (tackling by
//!   the side's aggression), the second covers, the rest mark the
//!   opponent in their zone goal-side.
//! - **Keeper:** positions on the ball–goal line, dives at shots on
//!   target, claims loose balls in his area, distributes.
//!
//! Every choice depends on the side's [`Tactics`](super::team::Tactics)
//! and the players' attributes. All of it is fixed point and uses the
//! match's RNG, so it is deterministic.

use super::ball::{self, Ball};
use super::engine::{Act, Cmd, CmdAct, KickKind, KickPlan, Match, N, Pad, Phase, SIDE, side_of};
use super::fx::{Fx, V2, V3};
use super::pitch::{self, End, GOAL_HALF_W, HALF_LEN, HALF_WID};
use super::rules::{self, Restart, RestartKind};
use super::team::{self, Role};

/// Speed a ground pass should still have at its receiver (m/s).
pub const PASS_ARRIVE: Fx = Fx::int(7);

/// How sharp the AI plays: error multiplier per difficulty for a side
/// facing a human (easy, normal, hard).
const ERROR: [Fx; 3] = [Fx::milli(1_700), Fx::milli(1_000), Fx::milli(650)];

/// How hard a side hunts the ball without it, per difficulty for a side
/// facing a human (easy, normal, hard); AI against AI plays Normal's
/// (issue #86). 0 is the old, patient pressing. With urgency `u`:
/// - the presser hunts `u` × [`PRESS_ZONE`] further up the pitch and
///   closes down anyone within 8 + `u` × [`PRESS_NEAR`] m;
/// - he tackles more from the front (× 1 + `u` × [`TACKLE_FRONT`]) and
///   less from behind, where a tackle is mostly a foul
///   (× 1 − `u` × [`TACKLE_BEHIND`]);
/// - the second chaser contests every loose ball, not only the other
///   side's;
/// - markers stand ball-side as well as goal-side of their man
///   ([`MARK_BALL_SIDE`]), in the passing lane to him.
const URGENCY: [Fx; 3] = [Fx::ZERO, Fx::ONE, Fx::milli(1_300)];
const PRESS_ZONE: Fx = Fx::int(14);
const PRESS_NEAR: Fx = Fx::int(6);
const TACKLE_FRONT: Fx = Fx::milli(600);
const TACKLE_BEHIND: Fx = Fx::milli(500);
const MARK_BALL_SIDE: Fx = Fx::milli(700);

pub fn urgency(m: &Match, s: usize) -> Fx {
    #[cfg(test)]
    if let Some(u) = m.urgency_test {
        return u[s];
    }
    if m.humans[s] {
        // A human side's teammates play like the AI at Normal.
        URGENCY[1]
    } else if !m.humans[1 - s] {
        URGENCY[1]
    } else {
        URGENCY[m.difficulty.min(2) as usize]
    }
}

fn err_scale(m: &Match, s: usize) -> Fx {
    if m.humans[s] || !m.humans[1 - s] {
        Fx::ONE
    } else {
        ERROR[m.difficulty.min(2) as usize]
    }
}

/// World ↔ team space: team space has the side attacking +x.
#[inline]
fn sign(m: &Match, s: usize) -> Fx {
    m.attacks(s).sign()
}

#[inline]
fn to_world(m: &Match, s: usize, p: V2) -> V2 {
    p.scale(sign(m, s))
}

#[inline]
fn to_team(m: &Match, s: usize, p: V2) -> V2 {
    p.scale(sign(m, s))
}

/// Opponents' offside line in side `s`'s attack-x.
pub fn offside_line(m: &Match, s: usize) -> Fx {
    let o = 1 - s;
    let mut xs = [Fx::ZERO; SIDE];
    let mut n = 0;
    for j in o * SIDE..o * SIDE + SIDE {
        if !m.players[j].off {
            xs[n] = m.ax(s, m.players[j].pos);
            n += 1;
        }
    }
    rules::second_last(&xs[..n])
}

/// Side `s`'s shape slot `k` for the ball at `ball` (world), in or out of
/// possession: the formation slid, squeezed and stretched.
pub fn anchor(m: &Match, s: usize, k: usize, ball: V2, attacking: bool) -> V2 {
    let t = &m.tactics[s];
    let f = t.formation();
    let base = f.base(k);
    let role = f.role(k);
    let b = to_team(m, s, ball);
    let w = Fx::ratio(t.width as i32, 100);
    let (x, y) = if attacking {
        let width = Fx::milli(820) + w.mul(Fx::milli(380));
        (
            base.x.mul(HALF_LEN).mul(Fx::milli(850)) + b.x.mul(Fx::milli(500)) + Fx::int(14),
            base.y.mul(HALF_WID).mul(width) + b.y.mul(Fx::milli(220)),
        )
    } else {
        let width = Fx::milli(600) + w.mul(Fx::milli(150));
        (
            base.x.mul(HALF_LEN).mul(Fx::milli(750)) + b.x.mul(Fx::milli(450)) - Fx::int(4),
            base.y.mul(HALF_WID).mul(width) + b.y.mul(Fx::milli(380)),
        )
    };
    let mut x = x;
    let line = Fx::ratio(t.line as i32, 100);
    match role {
        Role::Cb | Role::Fb if !attacking => {
            // The back line: flat, never behind the deepest it may go,
            // stepping up with the ball (the offside trap).
            let deepest = -HALF_LEN + Fx::int(7) + line.mul(Fx::int(14));
            let flat = b.x - Fx::int(16) - (Fx::ONE - line).mul(Fx::int(12));
            x = flat.max(deepest).min(Fx::int(8));
            if role == Role::Fb {
                x += Fx::int(1);
            }
        }
        Role::Fb if attacking => {
            // Overlap when the ball is on their wing.
            if (base.y.0 > 0) == (b.y.0 > 0) && b.x > -Fx::int(20) {
                x += Fx::int(12);
            }
        }
        Role::St | Role::W if attacking => {
            // Play on the last defender's shoulder: drift around the line
            // (2 m onside to 2 m off, a slow wave per player), the way
            // forwards do — and why offsides happen.
            let off = offside_line(m, s);
            let phase = ((m.tick as i32 + k as i32 * 53) % 240 - 120).abs();
            let drift = Fx::int(4).mul(Fx::ratio(phase, 120)) - Fx::int(2);
            // In the attacking third they push up to it.
            x = if b.x > Fx::int(15) {
                off + drift
            } else {
                x.min(off + drift)
            }
            .max(b.x - Fx::int(15));
        }
        Role::St if !attacking => {
            // The front man stays up for the counter.
            x = x.max(-Fx::int(8));
        }
        _ => {}
    }
    let x = x.clamp(-HALF_LEN + Fx::int(3), HALF_LEN - Fx::int(8));
    let y = y.clamp(-HALF_WID + Fx::int(2), HALF_WID - Fx::int(2));
    to_world(m, s, V2::new(x, y))
}

/// Seconds-ish a player needs to reach `p` (distance over top speed).
fn time_to(m: &Match, i: usize, p: V2) -> Fx {
    (p - m.players[i].pos).len().div(m.top_speed(i, true))
}

/// Ticks between trajectory samples, and how many.
pub const TRAJ_STEP: u32 = 3;
pub const TRAJ_LEN: usize = 24;

/// The free ball's path for the next 72 ticks, sampled every
/// [`TRAJ_STEP`]: computed once per tick and shared by every chaser and
/// both keepers (each used to predict on its own: ~740 ball steps a tick
/// on the N64, now 72).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Traj {
    pub at: [V3; TRAJ_LEN],
    /// Samples before the ball stops (or all of them).
    pub len: u8,
}

impl Default for Traj {
    fn default() -> Traj {
        Traj {
            at: [V3::ZERO; TRAJ_LEN],
            len: 0,
        }
    }
}

impl Traj {
    pub fn of(ball: &Ball, wet: bool) -> Traj {
        let mut t = Traj::default();
        let mut b = *ball;
        for k in 0..TRAJ_LEN {
            for _ in 0..TRAJ_STEP {
                b.step(wet);
            }
            t.at[k] = b.pos;
            t.len = k as u8 + 1;
            if b.vel.ground().shorter_than(Fx::milli(300))
                && b.pos.y <= pitch::BALL_R + Fx::milli(5)
            {
                break;
            }
        }
        t
    }

    /// When (ticks), where (y) and how high the ball crosses the plane of
    /// the goal line at `end`, if it does within the horizon.
    pub fn crossing(&self, from: V3, end: End) -> Option<(u32, Fx, Fx)> {
        let gx = end.goal_x();
        let s = end.sign();
        let mut prev = from;
        for k in 0..self.len as usize {
            let p = self.at[k];
            if (p.x - gx).mul(s) >= Fx::ZERO {
                // Interpolate between the samples around the plane.
                let a = (gx - prev.x).mul(s);
                let span = (p.x - prev.x).mul(s);
                let f = if span > Fx::ZERO {
                    a.div(span).clamp(Fx::ZERO, Fx::ONE)
                } else {
                    Fx::ONE
                };
                let y = prev.z + (p.z - prev.z).mul(f);
                let h = prev.y + (p.y - prev.y).mul(f);
                let t = (k as u32) * TRAJ_STEP
                    + (f.mul(Fx::int(TRAJ_STEP as i32))).round_int().max(1) as u32;
                return Some((t, y, h));
            }
            prev = p;
        }
        None
    }
}

/// Where the moving ball will be when player `i` can get there (from the
/// shared trajectory).
pub fn intercept_point(m: &Match, i: usize) -> V2 {
    let me = m.players[i].pos;
    let speed = m.top_speed(i, true);
    let t = &m.traj;
    for k in 0..t.len as usize {
        let p = t.at[k].ground();
        let ticks = Fx::int(((k as u32 + 1) * TRAJ_STEP) as i32);
        let reach = speed.mul(ticks).div(Fx::int(60));
        if (p - me).shorter_than(reach + Fx::milli(400)) {
            return pitch::clamp_to_surround(p);
        }
    }
    let last = if t.len > 0 {
        t.at[t.len as usize - 1].ground()
    } else {
        m.ball.pos.ground()
    };
    pitch::clamp_to_surround(last)
}

/// The side's player nearest the ball (by time), skipping `except`.
pub fn nearest_to_ball(m: &Match, s: usize, except: Option<u8>) -> Option<u8> {
    let b = m.ball.pos.ground();
    let mut best: Option<(usize, Fx)> = None;
    for i in s * SIDE..s * SIDE + SIDE {
        let p = &m.players[i];
        if p.off || p.role == Role::Gk || Some(i as u8) == except {
            continue;
        }
        let t = time_to(m, i, b);
        if best.is_none_or(|(_, bt)| t < bt) {
            best = Some((i, t));
        }
    }
    best.map(|(i, _)| i as u8)
}

/// The human's next player on a switch: the teammate nearest the ball
/// other than the one he has (the HUD's secondary marker).
pub fn switch_target(m: &Match, s: usize) -> Option<u8> {
    nearest_to_ball(m, s, Some(m.active[s]))
}

/// The first two chasers of a loose or opposing ball for side `s`.
fn chasers(m: &Match, s: usize) -> (Option<u8>, Option<u8>) {
    let a = nearest_to_ball(m, s, None);
    let b = nearest_to_ball(m, s, a);
    (a, b)
}

/// Every player's command this tick (the engine overrides the humans').
pub fn think(m: &mut Match, cmds: &mut [Option<Cmd>; N]) {
    m.traj = if m.owner.is_none() {
        Traj::of(&m.ball, m.wet)
    } else {
        Traj::default()
    };
    let poss = m.possession();
    let ball = m.ball.pos.ground();
    for s in 0..2 {
        let attacking = poss == Some(s)
            || (poss.is_none()
                && side_of(m.last_touch as usize) == s
                && m.ball.vel.ground().len() > Fx::int(3));
        let (c1, c2) = chasers(m, s);
        // Marking assignments: each opponent marked once, in index order.
        let mut marked = 0u32;
        for k in 0..SIDE {
            let i = s * SIDE + k;
            if m.players[i].off {
                continue;
            }
            if m.humans[s] && m.active[s] as usize == i {
                continue;
            }
            if !matches!(
                m.players[i].act,
                Act::Free | Act::Kick { .. } | Act::Tackle { .. }
            ) {
                continue;
            }
            let home = anchor(m, s, k, ball, attacking);
            let ball_dist = (ball - m.players[i].pos).len();
            let cmd = if m.players[i].role == Role::Gk {
                keeper(m, i, home)
            } else if m.owner == Some(i as u8) {
                carrier(m, i)
            } else if poss == Some(s) {
                support(m, i, home)
            } else if poss.is_none() && Some(i as u8) == m.receiver {
                let opp_near = opp_distance(m, s, m.players[i].pos) < Fx::milli(3_000);
                let ball_close = ball_dist < Fx::int(3);
                let hurry = opp_near || !ball_close;
                chase(m, i, hurry)
            } else if poss.is_none()
                && (Some(i as u8) == c1
                    || (Some(i as u8) == c2 && (!attacking || urgency(m, s) > Fx::ZERO)))
            {
                chase(m, i, true)
            } else if poss.is_some() && Some(i as u8) == c1 {
                press(m, i)
            } else if poss.is_some() && Some(i as u8) == c2 {
                cover(m, i)
            } else if !attacking {
                mark(m, i, home, &mut marked)
            } else {
                go_to(m, i, home, false)
            };
            if m.owner != Some(i as u8) {
                // (The carrier keeps the run he chose: `carrier` set it.)
                m.players[i].target = home;
            }
            cmds[i] = Some(cmd);
        }
    }
}

/// Walk or run to `p`, slowing on arrival.
fn go_to(m: &Match, i: usize, p: V2, hurry: bool) -> Cmd {
    let d = p - m.players[i].pos;
    let (dir, len) = d.norm_len();
    let urgency = (len.div(Fx::int(3))).min(Fx::ONE);
    if len < Fx::milli(400) {
        return Cmd::go(V2::ZERO, false);
    }
    Cmd::go(dir.scale(urgency), hurry && len > Fx::int(4))
}

fn chase(m: &Match, i: usize, hurry: bool) -> Cmd {
    let me = m.players[i].pos;
    let b = m.ball.pos.ground();
    // Close in: run straight through the ball (never brake short of it).
    let p = if (b - me).shorter_than(Fx::milli(2_500)) {
        b + m.ball.vel.ground().scale(Fx::milli(120))
    } else {
        intercept_point(m, i)
    };
    let dir = (p - me).norm();
    Cmd::go(
        if dir == V2::ZERO {
            m.players[i].face
        } else {
            dir
        },
        hurry,
    )
}

/// Close down the carrier goal-side; tackle when close.
fn press(m: &mut Match, i: usize) -> Cmd {
    let s = side_of(i);
    let o = m.owner.unwrap() as usize;
    let carrier = m.players[o];
    let own_goal = m.ends[s].goal_center();
    let me = m.players[i];
    let t = &m.tactics[s];
    let goal_side = carrier.pos + (own_goal - carrier.pos).norm().scale(Fx::milli(900));
    let d = (carrier.pos - me.pos).len();
    // Pressing zone: how far up the side hunts.
    let press = Fx::ratio(t.press as i32, 100);
    let u = urgency(m, s);
    let zone = -HALF_LEN + press.mul(Fx::int(85)) + Fx::int(10) + u.mul(PRESS_ZONE);
    let in_zone = m.ax(1 - s, carrier.pos) > -zone || d < Fx::int(8) + u.mul(PRESS_NEAR);
    if matches!(carrier.act, Act::Hold { .. }) {
        // A keeper with the ball: back off.
        return go_to(m, i, m.players[i].target, false);
    }
    if !in_zone {
        // Contain: hold a few meters goal-side.
        let p = carrier.pos + (own_goal - carrier.pos).norm().scale(Fx::int(5));
        return go_to(m, i, p, false);
    }
    let closing = d > Fx::milli(1_400);
    let mut cmd = go_to(m, i, goal_side, closing);
    cmd.mv = cmd.mv.norm();
    cmd.face = Some((carrier.pos - me.pos).norm());
    cmd.sprint = closing && me.stamina > Fx::milli(120);
    let aggr = Fx::ratio(t.aggression as i32, 100);
    let err = err_scale(m, s);
    // From behind a tackle is mostly a foul: an urgent presser goes in
    // from the front, and holds off once he is behind his man.
    let behind = me.face.dot(carrier.face) > Fx::milli(500);
    if d < Fx::milli(1_500) && me.cool == 0 {
        let p = (Fx::milli(70) + aggr.mul(Fx::milli(80))).div(err);
        let p = if behind {
            p.mul(Fx::ONE - u.mul(TACKLE_BEHIND).min(Fx::ONE))
        } else {
            p.mul(Fx::ONE + u.mul(TACKLE_FRONT))
        };
        if m.rng.chance(p) {
            cmd.act = Some(CmdAct::Tackle);
        }
    } else if d < Fx::milli(2_600)
        && d > Fx::milli(1_800)
        && carrier.vel.len() > Fx::int(5)
        && me.cool == 0
    {
        // A slide at a runner, from the front or side.
        let facing = (carrier.pos - me.pos).norm().dot(carrier.vel.norm());
        if facing < Fx::milli(200) && m.rng.chance(aggr.mul(Fx::milli(15))) {
            cmd.act = Some(CmdAct::Slide(
                (carrier.pos + carrier.vel.scale(Fx::milli(200)) - me.pos).norm(),
            ));
        }
    }
    cmd
}

/// Second defender: between the ball and goal, behind the presser.
fn cover(m: &Match, i: usize) -> Cmd {
    let s = side_of(i);
    let own_goal = m.ends[s].goal_center();
    let b = m.ball.pos.ground();
    let p = b + (own_goal - b).norm().scale(Fx::int(7));
    go_to(m, i, p, true)
}

/// Zone marking: the nearest unmarked opponent around the slot, goal-side.
fn mark(m: &Match, i: usize, home: V2, marked: &mut u32) -> Cmd {
    let s = side_of(i);
    let o = 1 - s;
    let own_goal = m.ends[s].goal_center();
    let mut best: Option<(usize, i64)> = None;
    for j in o * SIDE..o * SIDE + SIDE {
        let p = &m.players[j];
        if p.off || p.role == Role::Gk || *marked & (1 << j) != 0 {
            continue;
        }
        let d = (p.pos - home).len_sq_raw();
        let zone = Fx::int(11).0 as i64;
        if d < zone * zone && best.is_none_or(|(_, bd)| d < bd) {
            best = Some((j, d));
        }
    }
    if let Some((j, _)) = best {
        *marked |= 1 << j;
        let opp = m.players[j].pos;
        // Goal-side, and with urgency ball-side too: between his man and
        // the ball, to cut the pass to him.
        let u = urgency(m, s);
        let ball = m.ball.pos.ground();
        let side = (own_goal - opp).norm() + (ball - opp).norm().scale(u.mul(MARK_BALL_SIDE));
        let side = if side == V2::ZERO {
            (own_goal - opp).norm()
        } else {
            side.norm()
        };
        let mut p = opp + side.scale(Fx::milli(1_800));
        // The back four hold their line instead of following a forward
        // in behind it: he is offside if the pass comes (the trap), or
        // through on goal if they misjudge it.
        if matches!(m.players[i].role, Role::Cb | Role::Fb) {
            let (pt, ht) = (to_team(m, s, p), to_team(m, s, home));
            if pt.x < ht.x - Fx::int(1) {
                p = to_world(m, s, V2::new(ht.x - Fx::int(1), pt.y));
            }
        }
        return go_to(m, i, p, true);
    }
    go_to(m, i, home, false)
}

/// In possession, off the ball.
fn support(m: &mut Match, i: usize, home: V2) -> Cmd {
    let s = side_of(i);
    let me = m.players[i];
    let carrier = m.owner.map(|o| m.players[o as usize]);
    // Re-plan every 20 ticks (staggered): pick the clearest lane near home.
    let slot = (m.tick as usize + i * 3).is_multiple_of(20);
    let mut target = me.target;
    if slot || (target - home).len() > Fx::int(12) {
        target = home;
        if let Some(c) = carrier {
            let mut best = Fx::ZERO;
            for dy in [Fx::ZERO, Fx::int(5), -Fx::int(5), Fx::int(9), -Fx::int(9)] {
                let cand = pitch::clamp_to_field(home + V2::new(Fx::ZERO, dy));
                let lane = lane_clearance(m, s, c.pos, cand);
                let room = opp_distance(m, s, cand).min(Fx::int(8));
                let score = lane + room.mul(Fx::milli(500)) - dy.abs().mul(Fx::milli(100));
                if score > best {
                    best = score;
                    target = cand;
                }
            }
        }
    }
    // Forwards: when the carrier looks up in the middle third, a run in
    // behind (the through ball may find them).
    let role = me.role;
    if matches!(role, Role::St | Role::W)
        && let Some(c) = carrier
        && m.ax(s, c.pos) > -Fx::int(15)
    {
        let line = offside_line(m, s);
        let mine = m.ax(s, me.pos);
        let onside = mine < line;
        if onside && (m.tick as usize + i * 7) % 150 < 40 {
            let run = V2::new(line + Fx::int(10), to_team(m, s, me.pos).y);
            target = to_world(m, s, run);
            m.players[i].target = target;
            return go_to(m, i, pitch::clamp_to_field(target), true);
        }
    }
    let mut cmd = go_to(m, i, target, false);
    if let Some(c) = carrier {
        cmd.face = Some((c.pos - me.pos).norm());
    }
    cmd
}

/// Distance from the nearest opponent of `s` to the segment `a..b`
/// (capped at 8 m).
pub fn lane_clearance(m: &Match, s: usize, a: V2, b: V2) -> Fx {
    let o = 1 - s;
    let mut best = Fx::int(8);
    for j in o * SIDE..o * SIDE + SIDE {
        let p = &m.players[j];
        if p.off {
            continue;
        }
        best = best.min(p.pos.dist_to_segment(a, b));
    }
    best
}

/// Interception margin of a ground pass from `a` to `b` at mean speed
/// `v` (m/s): the least, over opponents, of the time he needs to reach the
/// ball's line minus the time the ball needs to get there (seconds,
/// capped at 2). Negative: someone gets there first.
pub fn lane_margin(m: &Match, s: usize, a: V2, b: V2, v: Fx) -> Fx {
    let o = 1 - s;
    let (dir, len) = (b - a).norm_len();
    let mut best = Fx::int(2);
    for j in o * SIDE..o * SIDE + SIDE {
        let p = &m.players[j];
        if p.off {
            continue;
        }
        let rel = p.pos - a;
        let along = rel.dot(dir).clamp(Fx::ZERO, len);
        let off = (rel - dir.scale(along)).len();
        // He needs to cover the gap less his reach; the ball, `along`.
        let t_him = (off - Fx::milli(700)).max(Fx::ZERO).div(Fx::int(7)) + Fx::milli(150);
        let t_ball = along.div(v.max(Fx::ONE));
        best = best.min(t_him - t_ball);
    }
    best
}

/// Distance from `p` to the nearest opponent of side `s` (capped 20 m).
pub fn opp_distance(m: &Match, s: usize, p: V2) -> Fx {
    let o = 1 - s;
    let mut best = Fx::int(20);
    for j in o * SIDE..o * SIDE + SIDE {
        let q = &m.players[j];
        if q.off {
            continue;
        }
        let d = q.pos - p;
        if d.x.abs() < best && d.y.abs() < best {
            best = best.min(d.len());
        }
    }
    best
}

/// How much a ball at `p` is worth to side `s` (0..~2.3): convex in the
/// progress up the pitch, plus the central attacking zone.
pub fn pitch_value(m: &Match, s: usize, p: V2) -> Fx {
    let a = ((m.ax(s, p) + HALF_LEN).div(HALF_LEN * 2)).clamp(Fx::ZERO, Fx::ONE);
    let mut v = a.mul(a).mul(Fx::int(2));
    if m.ax(s, p) > Fx::int(28) {
        let central = (Fx::ONE - p.y.abs().div(Fx::int(20))).max(Fx::ZERO);
        v += central.mul(Fx::milli(300));
    }
    v
}

/// The carrier's choice: the best of shooting, passing, crossing,
/// dribbling and clearing, by expected gain in [`pitch_value`] and risk.
fn carrier(m: &mut Match, i: usize) -> Cmd {
    let s = side_of(i);
    let me = m.players[i];
    if matches!(me.act, Act::Kick { .. }) {
        return Cmd::go(V2::ZERO, false);
    }
    let goal = m.attacks(s).goal_center();
    let ball = m.ball.pos.ground();
    let pressure = opp_distance(m, s, me.pos);
    let tactics = m.tactics[s];
    let tempo = Fx::ratio(tactics.tempo as i32, 100);
    let direct = Fx::ratio(tactics.direct as i32, 100);
    let err = err_scale(m, s);
    // Think every 4 ticks (staggered), or at once under pressure; between
    // thoughts keep running where he was going.
    let think_now = (m.tick as usize + i).is_multiple_of(4) || pressure < Fx::milli(1_800);
    if !think_now {
        let dir = m.players[i].target - me.pos;
        let sprint = m.players[i].sprint;
        return Cmd::go(dir.norm(), sprint);
    }
    // First touch: settle it before releasing it, unless pressed.
    let settled = m.tick.saturating_sub(m.owned_since) >= 10 || pressure < Fx::milli(2_500);
    let cur = pitch_value(m, s, ball);
    let ball_ax = m.ax(s, ball);
    let line = offside_line(m, s);
    let mut best_score = -Fx::int(9);
    // 1 shot, 2 pass, 3 through, 4 cross, 5 dribble (test counters).
    let mut best_kind = 0usize;
    let mut best: Option<Cmd> = None;
    let kick = |plan: KickPlan| Cmd {
        act: Some(CmdAct::Kick(plan)),
        ..Cmd::go(V2::ZERO, false)
    };

    // 1. Shoot.
    let gd = (goal - ball).len();
    if settled && gd < Fx::int(34) && ball_ax > Fx::int(15) {
        let open = shot_openness(m, s, ball);
        let shot = m.attr(i, team::SHOT);
        let dist_f = (Fx::int(37) - gd).div(Fx::int(25)).clamp(Fx::ZERO, Fx::ONE);
        let q = open
            .mul(dist_f)
            .mul(Fx::milli(500) + shot.mul(Fx::milli(700)));
        let score = q.mul(Fx::milli(4_800))
            + if gd < Fx::int(13) && open > Fx::milli(150) {
                Fx::int(2)
            } else {
                Fx::ZERO
            };
        if score > best_score && q > Fx::milli(60) {
            best_score = score;
            let power = Fx::milli(550) + m.rng.unit().mul(Fx::milli(350));
            let plan = shot_plan(m, i, None, power, err);
            best = Some(kick(plan));
            best_kind = 1;
        }
    }

    // 2. Pass (ground or lofted), and through balls behind the line.
    let mut box_runners = 0;
    let misjudge = (Fx::ONE - m.attr(i, team::PASS))
        .mul(Fx::milli(300))
        .mul(err);
    for k in 0..SIDE {
        let j = s * SIDE + k;
        let r = m.players[j];
        if j == i || r.off || matches!(r.act, Act::Down { .. }) {
            continue;
        }
        if (r.pos - m.attacks(s).pen_spot()).shorter_than(Fx::int(18)) {
            box_runners += 1;
        }
        if !settled {
            continue;
        }
        if r.role == Role::Gk && (pressure > Fx::int(3) || ball_ax > -Fx::int(20)) {
            continue;
        }
        // Not to a man standing offside — the AI sees the line, but a
        // poor passer sometimes misjudges it, and a tight call (within a
        // meter and a half) fools anyone half the time.
        let rax = m.ax(s, r.pos);
        if rules::offside_position(rax, ball_ax, line) {
            #[cfg(test)]
            {
                m.diag.chose[0] += 1;
            }
            let tight = rax - line.max(ball_ax) < Fx::milli(1_500);
            let p = if tight {
                misjudge + Fx::milli(450)
            } else {
                misjudge
            };
            if !m.rng.chance(p) {
                continue;
            }
        }
        // A through ball into the space behind the line: on when the
        // runner gets to the spot before any defender.
        if matches!(r.role, Role::St | Role::W | Role::Am)
            && rax > line - Fx::int(8)
            && rax <= line + Fx::int(1)
            && ball_ax > -Fx::int(25)
        {
            let depth = Fx::int(7) + m.rng.unit().mul(Fx::int(5));
            let ahead = pitch::clamp_to_field(to_world(
                m,
                s,
                V2::new(line + depth, to_team(m, s, r.pos).y.mul(Fx::milli(800))),
            ));
            let (to_spot, ga) = (ahead - r.pos).norm_len();
            // A runner already moving that way is ahead of the game.
            let momentum = r.vel.dot(to_spot).max(Fx::ZERO).mul(Fx::milli(300));
            let t_run = (ga - momentum).max(Fx::ZERO).div(m.top_speed(j, true)) + Fx::milli(200);
            let mut t_def = Fx::int(9);
            for q in (1 - s) * SIDE..(1 - s) * SIDE + SIDE {
                let o = &m.players[q];
                if !o.off {
                    let reach = if o.role == Role::Gk {
                        Fx::milli(1_500)
                    } else {
                        Fx::milli(700)
                    };
                    let (dir, dist) = (ahead - o.pos).norm_len();
                    // Reaction, and turning when the spot is behind him.
                    let turn = if o.face.dot(dir) < Fx::ZERO {
                        Fx::milli(350)
                    } else {
                        Fx::ZERO
                    };
                    let t =
                        (dist - reach).max(Fx::ZERO).div(Fx::milli(7_500)) + Fx::milli(250) + turn;
                    t_def = t_def.min(t);
                }
            }
            let d_ahead = (ahead - ball).len();
            let ground_ok = lane_margin(m, s, ball, ahead, Fx::int(12)) > Fx::ZERO;
            if t_run + Fx::milli(150) < t_def && d_ahead > Fx::int(10) && ga < Fx::int(18) {
                let gain = pitch_value(m, s, ahead) - cur;
                let score = gain.mul(Fx::int(3))
                    + Fx::milli(500)
                    + tempo.mul(Fx::milli(800))
                    + (t_def - t_run).min(Fx::ONE);
                if score > best_score {
                    best_score = score;
                    let plan = pass_plan(
                        m,
                        i,
                        ahead,
                        Some(j as u8),
                        !ground_ok,
                        KickKind::Through,
                        err,
                    );
                    best = Some(kick(plan));
                    best_kind = 3;
                }
            }
        }
        let lead = r.vel.scale(Fx::milli(600));
        let tgt = pitch::clamp_to_field(r.pos + lead);
        let d = (tgt - ball).len();
        if d < Fx::int(5) || d > Fx::int(48) {
            continue;
        }
        let room = opp_distance(m, s, tgt);
        let gain = pitch_value(m, s, tgt) - cur;
        let v0 = ball::pass_speed(d, PASS_ARRIVE).min(Fx::int(26));
        let margin = lane_margin(m, s, ball, tgt, (v0 + PASS_ARRIVE).mul(Fx::HALF));
        // Chip it when the ground lane is shut and it's far enough to
        // drop in space.
        let lofted = margin < Fx::milli(100)
            && d > Fx::int(22)
            && room > Fx::int(5)
            && gain > Fx::milli(150);
        if margin < Fx::milli(100) && !lofted {
            continue;
        }
        let safety = if lofted {
            ((room - Fx::int(4)).div(Fx::int(6)))
                .clamp(Fx::ZERO, Fx::ONE)
                .mul(Fx::milli(450))
        } else {
            ((margin + Fx::milli(100)).div(Fx::milli(700)))
                .clamp(Fx::ZERO, Fx::ONE)
                .mul(((room - Fx::ONE).div(Fx::int(4))).clamp(Fx::milli(250), Fx::ONE))
        };
        let progress_w = Fx::milli(2_500) + tempo.mul(Fx::milli(2_000));
        let mut score = (gain.mul(progress_w) + Fx::milli(350)).mul(safety)
            - (Fx::ONE - safety).mul(Fx::milli(800))
            - (d - Fx::int(30)).max(Fx::ZERO).mul(Fx::milli(30));
        if lofted {
            score = score.mul(Fx::milli(600) + direct.mul(Fx::milli(600)));
        }
        if score > best_score {
            best_score = score;
            let plan = pass_plan(m, i, tgt, Some(j as u8), lofted, KickKind::Pass, err);
            best = Some(kick(plan));
            best_kind = 2;
        }
    }

    // 3. Cross from wide in the final third.
    if settled && ball_ax > Fx::int(25) && me.pos.y.abs() > Fx::int(15) && box_runners >= 1 {
        let spot = m.attacks(s).pen_spot();
        let tgt = spot
            + V2::new(
                -m.attacks(s)
                    .sign()
                    .mul(Fx::int(2) - m.rng.unit().mul(Fx::int(4))),
                m.rng.signed().mul(Fx::int(4)),
            );
        // The deeper and wider, the more a cross is the ball to play.
        let depth = ((ball_ax - Fx::int(25)).div(Fx::int(20))).clamp(Fx::ZERO, Fx::ONE);
        let score = Fx::milli(1_100)
            + depth.mul(Fx::milli(900))
            + Fx::int(box_runners.min(4)).mul(Fx::milli(350));
        if score > best_score {
            best_score = score;
            let mut plan = pass_plan(m, i, tgt, None, true, KickKind::Cross, err);
            plan.spin = me
                .pos
                .y
                .signum()
                .mul(Fx::milli(180))
                .mul(m.attacks(s).sign());
            best = Some(kick(plan));
            best_kind = 4;
        }
    }

    // 4. Dribble into space.
    let fwd = (goal - me.pos).norm();
    let mut dir = fwd;
    let mut space = Fx::int(12);
    for j in (1 - s) * SIDE..(1 - s) * SIDE + SIDE {
        let o = &m.players[j];
        // A keeper come out at him is a man to go round, too.
        let keeper_out = o.role == Role::Gk && (o.pos - me.pos).shorter_than(Fx::int(7));
        if o.off || (o.role == Role::Gk && !keeper_out) {
            continue;
        }
        let d = o.pos - me.pos;
        if d.dot(dir) > -Fx::ONE && d.shorter_than(Fx::int(12)) {
            let l = d.len();
            if l < space {
                space = l;
            }
            if l < Fx::int(7) && d.dot(fwd) > Fx::ZERO {
                // Steer away from him, harder the closer he is.
                let side = if fwd.cross(d) > Fx::ZERO {
                    -Fx::ONE
                } else {
                    Fx::ONE
                };
                let k = (Fx::int(7) - l).div(Fx::int(7));
                dir = (dir + fwd.perp().scale(side.mul(k).mul(Fx::milli(1_300)))).norm();
            }
        }
    }
    // Keep off the touchline.
    if me.pos.y.abs() > HALF_WID - Fx::int(4) {
        dir = (dir + V2::new(Fx::ZERO, -me.pos.y.signum().mul(Fx::milli(700)))).norm();
    }
    let drib = m.attr(i, team::DRIBBLE);
    let ahead = pitch::clamp_to_field(me.pos + dir.scale(Fx::int(6)));
    let gain = pitch_value(m, s, ahead) - cur;
    let space_f = ((space - Fx::milli(1_500)).div(Fx::int(5))).clamp(Fx::ZERO, Fx::ONE);
    let dribble_score = (gain.mul(Fx::int(3)) + Fx::milli(250))
        .mul(space_f)
        .mul(Fx::milli(600) + drib.mul(Fx::milli(600)))
        - (Fx::ONE - space_f).mul(Fx::milli(500));
    if dribble_score > best_score || best.is_none() {
        best_score = dribble_score;
        m.players[i].target = me.pos + dir.scale(Fx::int(4));
        best = Some(Cmd::go(dir, space > Fx::int(4)));
        best_kind = 5;
    }

    // 5. Clear under pressure in the own third.
    if ball_ax < -Fx::int(28) && pressure < Fx::milli(2_200) && best_score < Fx::ZERO {
        let wing = if me.pos.y < Fx::ZERO {
            -Fx::ONE
        } else {
            Fx::ONE
        };
        let tgt = to_world(m, s, V2::new(Fx::int(5), wing.mul(Fx::int(25))));
        let plan = pass_plan(m, i, tgt, None, true, KickKind::Clear, err);
        #[cfg(test)]
        {
            m.diag.chose[6] += 1;
        }
        return kick(plan);
    }
    #[cfg(test)]
    {
        m.diag.chose[best_kind] += 1;
    }
    let _ = best_kind;
    best.unwrap_or_else(|| Cmd::go(fwd, false))
}

/// How open the goal looks from `p` for side `s` (0..1): the mouth's
/// angle, less the share blocked by bodies.
pub fn shot_openness(m: &Match, s: usize, p: V2) -> Fx {
    let end = m.attacks(s);
    let gx = end.goal_x();
    let left = V2::new(gx, GOAL_HALF_W) - p;
    let right = V2::new(gx, -GOAL_HALF_W) - p;
    let (ln, _) = left.norm_len();
    let (rn, _) = right.norm_len();
    // 1 - cos(angle) grows with the angle; a penalty-spot view (~37°) ≈ 0.2.
    let wide = (Fx::ONE - ln.dot(rn)).mul(Fx::int(5)).min(Fx::ONE);
    let mut blocked = Fx::ZERO;
    let o = 1 - s;
    for j in o * SIDE..o * SIDE + SIDE {
        let q = &m.players[j];
        if q.off {
            continue;
        }
        let d = q.pos.dist_to_segment(p, end.goal_center());
        if d < Fx::milli(1_200) {
            blocked += Fx::milli(300);
            // A keeper come out at the shooter fills more of the goal
            // the closer he is: he has closed the angle.
            let near = (q.pos - p).len();
            if q.role == Role::Gk && near < Fx::int(6) {
                blocked += (Fx::int(6) - near).mul(Fx::milli(200));
            }
        }
    }
    (wide - blocked).max(Fx::ZERO)
}

/// A pass from player `i` to `tgt`, along the ground or lofted, with the
/// passer's error.
pub fn pass_plan(
    m: &mut Match,
    i: usize,
    tgt: V2,
    to: Option<u8>,
    lofted: bool,
    kind: KickKind,
    err: Fx,
) -> KickPlan {
    let from = m.ball.pos.ground();
    let (dir, d) = (tgt - from).norm_len();
    let skill = m.attr(i, team::PASS);
    let pressure = opp_distance(m, side_of(i), m.players[i].pos);
    let press_f = if pressure < Fx::int(2) {
        Fx::milli(1_600)
    } else {
        Fx::ONE
    };
    let wobble = (Fx::ONE - skill).mul(Fx::milli(160)).mul(err).mul(press_f) + Fx::milli(8);
    let dir = (dir + dir.perp().scale(m.rng.signed().mul(wobble))).norm();
    let (speed, loft) = if lofted {
        // ~35°: v² = g·d / sin(70°), plus air drag.
        let v = (Fx::milli(10_440).mul(d)).sqrt().mul(Fx::milli(1_120));
        (v.clamp(Fx::int(9), Fx::int(30)), Fx::milli(570))
    } else {
        let arrive = if kind == KickKind::Through {
            Fx::int(3)
        } else {
            PASS_ARRIVE
        };
        (
            ball::pass_speed(d, arrive).clamp(Fx::int(6), Fx::int(26)),
            Fx::ZERO,
        )
    };
    let speed = speed.mul(Fx::ONE + m.rng.signed().mul(wobble).mul(Fx::milli(400)));
    KickPlan {
        kind,
        vel: ball::launch(dir, speed, loft),
        spin: Fx::ZERO,
        to,
    }
}

/// A shot from player `i`: `aim` = the target y on the goal line (None:
/// the corner away from the keeper), `power` 0..1.
pub fn shot_plan(m: &mut Match, i: usize, aim: Option<Fx>, power: Fx, err: Fx) -> KickPlan {
    let s = side_of(i);
    let end = m.attacks(s);
    let from = m.ball.pos.ground();
    let keeper = m.players[(1 - s) * SIDE].pos;
    let y = aim.unwrap_or_else(|| {
        let away = if keeper.y > Fx::ZERO {
            -Fx::ONE
        } else {
            Fx::ONE
        };
        away.mul(GOAL_HALF_W - Fx::milli(450))
    });
    let target = V2::new(end.goal_x(), y);
    let (dir, d) = (target - from).norm_len();
    let skill = m.attr(i, team::SHOT);
    let tired = Fx::ONE + (Fx::ONE - m.players[i].stamina).mul(Fx::milli(500));
    let pressure = opp_distance(m, s, m.players[i].pos);
    let press_f = if pressure < Fx::milli(1_500) {
        Fx::milli(1_500)
    } else {
        Fx::ONE
    };
    let wobble = ((Fx::ONE - skill).mul(Fx::milli(110)) + power.mul(Fx::milli(30)))
        .mul(err)
        .mul(tired)
        .mul(press_f)
        + Fx::milli(10);
    let dir = (dir + dir.perp().scale(m.rng.signed().mul(wobble))).norm();
    let speed = Fx::int(17) + power.mul(Fx::int(10)) + skill.mul(Fx::int(6));
    // Over-hit shots rise: loft grows with power and distance.
    let loft = Fx::milli(40)
        + power.mul(Fx::milli(90))
        + d.mul(Fx::milli(3))
        + m.rng.unit().mul(wobble).mul(Fx::milli(600));
    // Curl back toward the target when aiming wide of center.
    let spin = -y.signum().mul(Fx::milli(120)).mul(skill).mul(end.sign());
    KickPlan {
        kind: KickKind::Shot,
        vel: ball::launch(dir, speed, loft),
        spin,
        to: None,
    }
}

/// The keeper: on the ball–goal line, sharp at shots, out for loose balls
/// in his area.
fn keeper(m: &mut Match, i: usize, _home: V2) -> Cmd {
    let s = side_of(i);
    let end = m.ends[s];
    let me = m.players[i];
    let b = m.ball.pos.ground();
    let goal = end.goal_center();
    // A shot (or any ball) heading in: dive if it can't be walked to.
    if m.owner.is_none()
        && let Some((t, y, h)) = m.traj.crossing(m.ball.pos, end)
        && y.abs() < GOAL_HALF_W + Fx::milli(600)
        && h < pitch::GOAL_H + Fx::milli(300)
    {
        let spot = V2::new(end.goal_x() - end.sign().mul(Fx::milli(700)), y);
        let gap = (spot - me.pos).len();
        let tt = Fx::int(t as i32).div(Fx::int(60));
        let walk = m.top_speed(i, true).mul(tt);
        let reflex = m.attr(i, team::DEFENSE);
        // Reaction time: he can't move for the first ticks of the shot.
        let react = (Fx::int(20) - reflex.mul(Fx::int(9))).round_int().max(6) as u32;
        let since = m.tick.saturating_sub(m.shot_tick);
        if since < react {
            return Cmd::go(V2::ZERO, false);
        }
        if gap > walk + Fx::milli(400) && !matches!(me.act, Act::Dive { .. }) {
            // He reads the line with an error that shrinks with reflexes
            // (and the AI's difficulty).
            let miss = (Fx::ONE - reflex)
                .mul(Fx::milli(2_200))
                .mul(err_scale(m, s));
            let spot = spot + V2::new(Fx::ZERO, m.rng.signed().mul(miss));
            return Cmd {
                act: Some(CmdAct::Dive(spot)),
                ..Cmd::go(V2::ZERO, false)
            };
        }
        let mut c = go_to(m, i, spot, true);
        c.face = Some((b - me.pos).norm());
        return c;
    }
    // A 1v1: an attacker with the ball in the box and nobody goal-side
    // of him. The keeper decides once (his handling, and how well he is
    // set on the ball's line) whether to stay or come out.
    if let Some(o) = m.owner
        && side_of(o as usize) != s
        && one_on_one(m, o as usize)
    {
        let key = m.owned_since + 1;
        if m.rush[s].0 != key {
            let c = m.players[o as usize].pos;
            let line = (c - goal).norm().dot((me.pos - goal).norm()).max(Fx::ZERO);
            let rating = m.attr(i, team::DEFENSE);
            let p = (Fx::milli(250) + (rating - Fx::milli(700)).mul(RUSH_PER_POINT))
                .mul(Fx::milli(500) + line.mul(Fx::milli(500)))
                .clamp(Fx::milli(50), Fx::milli(800));
            let out = m.rng.chance(p);
            m.rush[s] = (key, out);
            #[cfg(test)]
            if out {
                m.diag.rushes += 1;
            }
        }
        if m.rush[s].1 {
            let c = m.players[o as usize];
            // Off his line at the ball: close the angle, then go down at
            // his feet once in reach.
            let d = (b - me.pos).len();
            if d < Fx::milli(2_400) && me.cool == 0 && matches!(me.act, Act::Free) {
                return Cmd {
                    act: Some(CmdAct::Dive(b + c.vel.scale(Fx::milli(150)))),
                    ..Cmd::go(V2::ZERO, false)
                };
            }
            // Meet him on the ball's line, never further out than the
            // penalty spot (he narrows the angle; his hands only count in
            // his area).
            let mut p = b + (goal - b).norm().scale(Fx::milli(1_200));
            let depth = end.sign().mul(end.goal_x() - p.x);
            if depth > RUSH_DEPTH {
                p = goal + (p - goal).scale(RUSH_DEPTH.div(depth.max(Fx::ONE)));
            }
            let mut cmd = go_to(m, i, p, true);
            cmd.mv = cmd.mv.norm();
            cmd.sprint = true;
            cmd.face = Some((b - me.pos).norm());
            return cmd;
        }
    } else {
        m.rush[s] = (0, false);
    }
    // Loose in his area and he's the closest: claim it.
    if m.owner.is_none()
        && pitch::in_penalty_area(b, end)
        && m.ball.vel.ground().len() < Fx::int(12)
    {
        let mine = time_to(m, i, b);
        let theirs =
            nearest_to_ball(m, 1 - s, None).map_or(Fx::int(99), |o| time_to(m, o as usize, b));
        if mine < theirs + Fx::milli(200) {
            return chase(m, i, true);
        }
    }
    // Position: on the line from goal to ball, further out as the ball
    // nears.
    let to_ball = b - goal;
    let (dir, dist) = to_ball.norm_len();
    let out = (dist.mul(Fx::milli(110))).clamp(Fx::milli(800), Fx::milli(5_500));
    let mut p = goal + dir.scale(out);
    p.y =
        p.y.clamp(-GOAL_HALF_W - Fx::int(1), GOAL_HALF_W + Fx::int(1));
    let mut c = go_to(m, i, p, false);
    c.face = Some(dir);
    c
}

/// How far off his line a keeper comes to meet a 1v1 (the penalty spot).
const RUSH_DEPTH: Fx = Fx::int(11);
/// A keeper of the typical 70 comes out of a quarter of 1v1s (when set
/// on the ball's line); each effective point of handling above adds this.
const RUSH_PER_POINT: Fx = Fx::milli(800);

/// Attacker `o` has the ball bearing down on the box he attacks (in it,
/// or within 26 m of goal and central) with no outfield defender
/// goal-side of him: one against the keeper.
pub fn one_on_one(m: &Match, o: usize) -> bool {
    let s = side_of(o);
    let p = m.players[o].pos;
    let goal = m.attacks(s).goal_center();
    let near = (goal - p).shorter_than(Fx::int(26)) && p.y.abs() < pitch::PEN_AREA_HALF_W;
    if !near && !pitch::in_penalty_area(p, m.attacks(s)) {
        return false;
    }
    let me = m.ax(s, p);
    let d = 1 - s;
    !(d * SIDE..d * SIDE + SIDE).any(|j| {
        let q = &m.players[j];
        !q.off && q.role != Role::Gk && m.ax(s, q.pos) > me - Fx::ONE
    })
}

/// A keeper with the ball in his hands: throw short to a free defender, or
/// kick long.
/// A human keeper's kick from his hands: a throw to the teammate the stick
/// points at (pass), a punt upfield (shoot: higher and longer with the
/// charge), or a long roll into space (through).
pub fn human_keeper_plan(m: &mut Match, i: usize, pad: &Pad) -> Option<KickPlan> {
    let s = side_of(i);
    let dir = if pad.mv == V2::ZERO {
        V2::new(m.attacks(s).sign(), Fx::ZERO)
    } else {
        pad.mv.norm()
    };
    if let Some(charge) = pad.pass {
        // To a teammate's run (he's told to come for it), or into space.
        let to = pass_target(m, i, dir);
        let tgt = match to {
            Some(j) => {
                let r = &m.players[j as usize];
                r.pos + r.vel.scale(Fx::HALF)
            }
            None => m.players[i].pos + dir.scale(Fx::int(14) + charge.mul(Fx::int(14))),
        };
        let mut plan = pass_plan(
            m,
            i,
            pitch::clamp_to_field(tgt),
            to,
            false,
            KickKind::Distribute,
            Fx::ZERO,
        );
        // A throw: a little above the turf.
        plan.vel.y = Fx::milli(1_500);
        return Some(plan);
    }
    if let Some(charge) = pad.shoot {
        let tgt = m.players[i].pos + dir.scale(Fx::int(35) + charge.mul(Fx::int(25)));
        let mut plan = pass_plan(
            m,
            i,
            pitch::clamp_to_field(tgt),
            None,
            true,
            KickKind::Distribute,
            Fx::ZERO,
        );
        plan.vel.y = plan.vel.y.max(Fx::int(9));
        return Some(plan);
    }
    if let Some(charge) = pad.through {
        let tgt = m.players[i].pos + dir.scale(Fx::int(18) + charge.mul(Fx::int(18)));
        return Some(pass_plan(
            m,
            i,
            pitch::clamp_to_field(tgt),
            None,
            false,
            KickKind::Distribute,
            Fx::ZERO,
        ));
    }
    None
}

pub fn keeper_distribution(m: &mut Match, i: usize) -> KickPlan {
    let s = side_of(i);
    let err = err_scale(m, s);
    let mut best: Option<(usize, Fx)> = None;
    for j in s * SIDE + 1..s * SIDE + SIDE {
        let r = &m.players[j];
        if r.off {
            continue;
        }
        let d = (r.pos - m.players[i].pos).len();
        if d < Fx::int(8) || d > Fx::int(35) {
            continue;
        }
        let room = opp_distance(m, s, r.pos);
        let lane = lane_clearance(m, s, m.players[i].pos, r.pos);
        if room > Fx::int(7) && lane > Fx::int(3) {
            let score = room + lane;
            if best.is_none_or(|(_, b)| score > b) {
                best = Some((j, score));
            }
        }
    }
    let direct = m.tactics[s].direct;
    if let Some((j, _)) = best
        && direct < 70
    {
        let tgt = m.players[j].pos;
        let mut plan = pass_plan(m, i, tgt, Some(j as u8), false, KickKind::Distribute, err);
        // A throw: a little above the turf.
        plan.vel.y = Fx::milli(1_500);
        return plan;
    }
    // Punt to the forwards.
    let y = m.rng.signed().mul(Fx::int(15));
    let tgt = to_world(m, s, V2::new(Fx::int(8), y));
    let mut plan = pass_plan(m, i, tgt, None, true, KickKind::Distribute, err);
    plan.vel.y = plan.vel.y.max(Fx::int(9));
    plan
}

// ----------------------------------------------------------- set pieces

/// The kick-off formation spot of player `i` (team in its own half;
/// the side not kicking off outside the center circle).
pub fn kickoff_spot(m: &Match, i: usize, kicking: bool) -> V2 {
    let s = side_of(i);
    let k = i % SIDE;
    let f = m.tactics[s].formation();
    let base = f.base(k);
    let x = base
        .x
        .mul(HALF_LEN)
        .mul(Fx::milli(860))
        .min(-Fx::milli(1_500));
    let y = base.y.mul(HALF_WID).mul(Fx::milli(800));
    let mut p = V2::new(x, y);
    // The side not kicking off waits outside the center circle (the
    // kicking side may stand in it). This was `(!kicking || ..) && inside &&
    // !kicking`, whose middle terms could never change the result
    // (clippy::overly_complex_bool_expr): the same test, written plainly.
    if !kicking && p.shorter_than(pitch::CIRCLE_R + Fx::int(1)) {
        p = p.norm().scale(pitch::CIRCLE_R + Fx::int(1));
        if p == V2::ZERO {
            p = V2::new(-pitch::CIRCLE_R - Fx::int(1), Fx::ZERO);
        }
    }
    to_world(m, s, p)
}

/// Who takes `r`.
pub fn pick_taker(m: &Match, r: Restart) -> u8 {
    let s = r.team as usize;
    let range = s * SIDE..s * SIDE + SIDE;
    let able = |j: &usize| !m.players[*j].off && m.players[*j].role != Role::Gk;
    match r.kind {
        // The keeper, or the nearest teammate once he's sent off.
        RestartKind::GoalKick => m
            .keeper_of(s)
            .map_or_else(|| nearest(m, s, r.spot), |k| k as u8),
        RestartKind::KickOff => {
            let f = m.tactics[s].formation();
            (0..SIDE)
                .rev()
                .find(|&k| f.role(k) == Role::St && !m.players[s * SIDE + k].off)
                .or_else(|| (1..SIDE).rev().find(|&k| !m.players[s * SIDE + k].off))
                .map_or(s * SIDE + 9, |k| s * SIDE + k) as u8
        }
        RestartKind::Penalty => range
            .filter(able)
            .max_by_key(|&j| (m.players[j].attrs[team::SHOT], usize::MAX - j))
            .unwrap_or(s * SIDE + 9) as u8,
        RestartKind::FreeKick { direct } => {
            let goal = m.attacks(s).goal_center();
            if direct && (goal - r.spot).shorter_than(Fx::int(30)) {
                range
                    .filter(able)
                    .max_by_key(|&j| (m.players[j].attrs[team::SHOT], usize::MAX - j))
                    .unwrap_or(s * SIDE + 9) as u8
            } else {
                nearest(m, s, r.spot)
            }
        }
        RestartKind::ThrowIn | RestartKind::Corner => nearest(m, s, r.spot),
    }
}

fn nearest(m: &Match, s: usize, p: V2) -> u8 {
    let mut best = (s * SIDE + 9, i64::MAX);
    for j in s * SIDE + 1..s * SIDE + SIDE {
        if m.players[j].off {
            continue;
        }
        let d = (m.players[j].pos - p).len_sq_raw();
        if d < best.1 {
            best = (j, d);
        }
    }
    best.0 as u8
}

/// Where player `i` stands for restart `r`.
pub fn restart_spot(m: &Match, i: usize, r: Restart) -> V2 {
    let s = side_of(i);
    let k = i % SIDE;
    let taking = s == r.team as usize;
    if i == m.taker as usize {
        // Behind the ball (on the line for a throw-in): he walks there.
        return m.taker_stance(r).0;
    }
    if r.kind == RestartKind::KickOff {
        return kickoff_spot(m, i, taking);
    }
    let own_end = m.ends[s];
    if m.players[i].role == Role::Gk {
        // On his line for penalties; in his area otherwise.
        let gx = own_end.goal_x()
            - own_end
                .sign()
                .mul(Fx::milli(if r.kind == RestartKind::Penalty {
                    300
                } else {
                    1_500
                }));
        let y = (r.spot.y.mul(Fx::milli(150))).clamp(-Fx::int(2), Fx::int(2));
        return V2::new(gx, y);
    }
    let attack_end = m.attacks(r.team as usize);
    let p = match r.kind {
        RestartKind::Penalty => {
            // Outside the area and arc, spread along its edge.
            let e = attack_end;
            let x = e.goal_x()
                - e.sign()
                    .mul(pitch::PEN_AREA_DEPTH + Fx::int(2) + Fx::int((k % 3) as i32));
            let y = Fx::int((k as i32 - 5) * 3);
            V2::new(x, y)
        }
        RestartKind::Corner => {
            let e = attack_end;
            let gx = e.goal_x();
            let into = -e.sign();
            if taking {
                match m.players[i].role {
                    Role::Cb | Role::St | Role::Am | Role::W => {
                        // Attack the box: near post, far post, penalty spot.
                        let spots = [(6, -3), (7, 3), (11, 0), (9, -6), (10, 6), (13, 2)];
                        let (dx, dy) = spots[k % spots.len()];
                        V2::new(
                            gx + into.mul(Fx::int(dx)),
                            Fx::int(dy).mul(r.spot.y.signum()),
                        )
                    }
                    _ => anchor(m, s, k, r.spot, true),
                }
            } else if matches!(m.players[i].role, Role::Cb | Role::Fb | Role::Dm) {
                // Zonal in the six-yard box and penalty spot.
                let spots = [(3, -2), (4, 2), (6, -4), (6, 4), (9, 0), (11, -3), (11, 3)];
                let (dx, dy) = spots[k % spots.len()];
                V2::new(gx + into.mul(Fx::int(dx)), Fx::int(dy))
            } else {
                anchor(m, s, k, r.spot, false)
            }
        }
        RestartKind::FreeKick { .. }
            if !taking && (attack_end.goal_center() - r.spot).shorter_than(Fx::int(32)) =>
        {
            // A wall of up to four on the line to the goal center.
            let goal = attack_end.goal_center();
            let dir = (goal - r.spot).norm();
            let wall = r.spot + dir.scale(pitch::CIRCLE_R);
            let slot = k as i32 - 5;
            if (0..4).contains(&slot) {
                wall + dir
                    .perp()
                    .scale(Fx::milli(650).mul(Fx::int(slot) - Fx::milli(1_500)))
            } else {
                anchor(m, s, k, r.spot, false)
            }
        }
        _ => anchor(m, s, k, r.spot, taking),
    };
    let mut p = pitch::clamp_to_field(p);
    // Opponents keep their distance.
    if !taking {
        let d = p - r.spot;
        let min = r.kind.distance() + Fx::milli(300);
        if d.shorter_than(min) {
            let n = if d == V2::ZERO {
                V2::new(own_end.sign(), Fx::ZERO)
            } else {
                d.norm()
            };
            p = pitch::clamp_to_field(r.spot + n.scale(min));
        }
        if r.kind == RestartKind::GoalKick && pitch::in_penalty_area(p, attack_end.other()) {
            p.x = attack_end.other().goal_x()
                - attack_end
                    .other()
                    .sign()
                    .mul(pitch::PEN_AREA_DEPTH + Fx::int(1));
        }
    }
    let _ = End::West;
    p
}

/// The AI taker's kick at a restart.
pub fn restart_kick(m: &mut Match, k: usize) -> KickPlan {
    let s = side_of(k);
    let r = m.restart;
    let err = err_scale(m, s);
    let goal = m.attacks(s).goal_center();
    match r.kind {
        RestartKind::Penalty => {
            let side = if m.rng.chance(Fx::HALF) {
                Fx::ONE
            } else {
                -Fx::ONE
            };
            let aim = side.mul(GOAL_HALF_W - Fx::milli(900) - m.rng.unit().mul(Fx::milli(800)));
            shot_plan(m, k, Some(aim), Fx::milli(500), err.mul(Fx::milli(700)))
        }
        RestartKind::Corner => {
            let into = -m.attacks(s).sign();
            let tgt = V2::new(
                goal.x + into.mul(Fx::int(7) + m.rng.unit().mul(Fx::int(5))),
                m.rng.signed().mul(Fx::int(5)),
            );
            let mut plan = pass_plan(m, k, tgt, None, true, KickKind::Cross, err);
            plan.spin = r.spot.y.signum().mul(Fx::milli(200)).mul(into);
            plan
        }
        RestartKind::FreeKick { direct } if direct && (goal - r.spot).shorter_than(Fx::int(27)) => {
            let mut plan = shot_plan(m, k, None, Fx::milli(450), err);
            // Over the wall, bending.
            plan.vel = ball::launch(
                V2::new(plan.vel.x, plan.vel.z).norm(),
                plan.vel.len(),
                Fx::milli(230),
            );
            plan
        }
        RestartKind::ThrowIn => {
            let to = nearest_free(m, s, r.spot, Fx::int(18), Some(k));
            let tgt = to.map_or(
                r.spot
                    + V2::new(
                        m.attacks(s).sign().mul(Fx::int(8)),
                        -r.spot.y.signum().mul(Fx::int(4)),
                    ),
                |j| m.players[j as usize].pos,
            );
            let (dir, d) = (tgt - r.spot).norm_len();
            let speed = (Fx::int(6) + d.mul(Fx::milli(450))).min(Fx::int(14));
            KickPlan {
                kind: KickKind::Throw,
                vel: ball::launch(dir, speed, Fx::milli(350)),
                spin: Fx::ZERO,
                to,
            }
        }
        RestartKind::KickOff => {
            let to = nearest_free(m, s, r.spot, Fx::int(20), Some(k));
            let tgt = to.map_or(
                r.spot - V2::new(m.attacks(s).sign().mul(Fx::int(8)), Fx::ZERO),
                |j| m.players[j as usize].pos,
            );
            pass_plan(m, k, tgt, to, false, KickKind::Pass, Fx::milli(500))
        }
        RestartKind::GoalKick => {
            if m.tactics[s].direct > 55 || m.rng.chance(Fx::milli(400)) {
                let y = m.rng.signed().mul(Fx::int(20));
                let tgt = to_world(m, s, V2::new(Fx::int(5), y));
                pass_plan(m, k, tgt, None, true, KickKind::Pass, err)
            } else {
                let to = nearest_free(m, s, r.spot, Fx::int(30), Some(k));
                let tgt = to.map_or(to_world(m, s, V2::new(-Fx::int(30), Fx::int(20))), |j| {
                    m.players[j as usize].pos
                });
                pass_plan(m, k, tgt, to, false, KickKind::Pass, err)
            }
        }
        RestartKind::FreeKick { .. } => {
            let to = nearest_free(m, s, r.spot, Fx::int(30), Some(k));
            let tgt = to.map_or(
                r.spot + V2::new(m.attacks(s).sign().mul(Fx::int(15)), Fx::ZERO),
                |j| m.players[j as usize].pos,
            );
            pass_plan(m, k, tgt, to, false, KickKind::Pass, err)
        }
    }
}

/// A teammate within `range` of `p` with room (most room first).
fn nearest_free(m: &Match, s: usize, p: V2, range: Fx, except: Option<usize>) -> Option<u8> {
    let mut best: Option<(usize, Fx)> = None;
    for j in s * SIDE + 1..s * SIDE + SIDE {
        if m.players[j].off || Some(j) == except {
            continue;
        }
        let d = (m.players[j].pos - p).len();
        if d > range || d < Fx::int(3) {
            continue;
        }
        let room = opp_distance(m, s, m.players[j].pos) - d.mul(Fx::milli(150));
        if best.is_none_or(|(_, b)| room > b) {
            best = Some((j, room));
        }
    }
    best.map(|(j, _)| j as u8)
}

// --------------------------------------------------------------- humans

/// The teammate a human pass in direction `dir` would go to: the best
/// aligned one in a 60° cone, nearer preferred.
pub fn pass_target(m: &Match, i: usize, dir: V2) -> Option<u8> {
    let s = side_of(i);
    let dir = if dir == V2::ZERO {
        m.players[i].face
    } else {
        dir.norm()
    };
    let from = m.players[i].pos;
    let mut best: Option<(usize, Fx)> = None;
    for j in s * SIDE..s * SIDE + SIDE {
        if j == i || m.players[j].off {
            continue;
        }
        let (n, d) = (m.players[j].pos - from).norm_len();
        if d < Fx::int(3) || d > Fx::int(50) {
            continue;
        }
        let c = n.dot(dir);
        if c < Fx::milli(500) {
            continue;
        }
        let score = c.mul(Fx::int(3)) - d.mul(Fx::milli(40));
        if best.is_none_or(|(_, b)| score > b) {
            best = Some((j, score));
        }
    }
    best.map(|(j, _)| j as u8)
}

/// What a human's released button means for player `i` (with the ball or
/// at a restart).
pub fn human_kick_plan(m: &mut Match, i: usize, pad: &Pad) -> Option<KickPlan> {
    let s = side_of(i);
    let dir = if pad.mv == V2::ZERO {
        m.players[i].face
    } else {
        pad.mv.norm()
    };
    let throw_in =
        m.phase != Phase::Play && m.restart.kind == RestartKind::ThrowIn && m.taker as usize == i;
    if throw_in && let Some(charge) = pad.shoot {
        let tgt = m.players[i].pos + dir.scale(Fx::int(18) + charge.mul(Fx::int(14)));
        let to = pass_target(m, i, dir);
        let mut p = pass_plan(
            m,
            i,
            pitch::clamp_to_field(tgt),
            to,
            false,
            KickKind::Throw,
            Fx::milli(500),
        );
        let (d, _sp) = V2::new(p.vel.x, p.vel.z).norm_len();
        p.vel = ball::launch(
            d,
            (Fx::int(11) + charge.mul(Fx::int(7))).min(Fx::int(18)),
            Fx::milli(400),
        );
        return Some(p);
    }
    if let Some(charge) = pad.pass.or(if throw_in { pad.through } else { None }) {
        let to = pass_target(m, i, dir);
        let plan = match to {
            Some(j) => {
                let r = m.players[j as usize];
                let tgt = pitch::clamp_to_field(r.pos + r.vel.scale(Fx::milli(500)));
                let mut p = pass_plan(m, i, tgt, to, false, KickKind::Pass, Fx::milli(700));
                p.vel = p.vel.scale(Fx::ONE + charge.mul(Fx::milli(300)));
                p
            }
            None => KickPlan {
                kind: KickKind::Pass,
                vel: ball::launch(dir, Fx::int(8) + charge.mul(Fx::int(16)), Fx::ZERO),
                spin: Fx::ZERO,
                to: None,
            },
        };
        if throw_in {
            let (d, sp) = V2::new(plan.vel.x, plan.vel.z).norm_len();
            return Some(KickPlan {
                kind: KickKind::Throw,
                vel: ball::launch(d, sp.min(Fx::int(14)), Fx::milli(350)),
                ..plan
            });
        }
        return Some(plan);
    }
    if let Some(charge) = pad.through {
        let to = pass_target(m, i, dir);
        let fwd = m.attacks(s).sign();
        let tgt = match to {
            Some(j) => {
                let r = m.players[j as usize];
                r.pos + V2::new(fwd.mul(Fx::int(7)), Fx::ZERO) + r.vel.scale(Fx::milli(400))
            }
            None => m.players[i].pos + dir.scale(Fx::int(20) + charge.mul(Fx::int(20))),
        };
        let lofted = charge > Fx::milli(450)
            || m.restart.kind == RestartKind::Corner && m.phase != Phase::Play;
        let kind = if m.phase != Phase::Play && m.restart.kind == RestartKind::Corner {
            KickKind::Cross
        } else {
            KickKind::Through
        };
        return Some(pass_plan(
            m,
            i,
            pitch::clamp_to_field(tgt),
            to,
            lofted,
            kind,
            Fx::milli(700),
        ));
    }
    if let Some(charge) = pad.shoot {
        // Aim across the mouth with the stick's sideways (world y) part;
        // a neutral stick picks the corner away from the keeper.
        let aim = (pad.mv.y.abs() > Fx::milli(200)).then(|| {
            pad.mv
                .y
                .clamp(-Fx::ONE, Fx::ONE)
                .mul(GOAL_HALF_W - Fx::milli(600))
        });
        return Some(shot_plan(m, i, aim, charge, Fx::milli(800)));
    }
    None
}

/// Where a kick with `plan` from `from` goes, `ticks` ahead (the HUD's
/// trajectory arc).
pub fn preview(from: V3, plan: &KickPlan, ticks: u32, wet: bool) -> Ball {
    let b = Ball {
        pos: from,
        vel: plan.vel,
        spin: plan.spin,
    };
    ball::predict(&b, ticks, wet)
}
