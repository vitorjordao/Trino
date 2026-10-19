//! Whole-match tests: invariants of AI-vs-AI matches, determinism, and
//! the rules as they happen in play.

extern crate std;
use std::vec::Vec;

use super::engine::MAX_SUBS;
use super::engine::*;
use super::fx::{Fx, V2};
use super::team::{self, CLUBS};

pub fn setup(home: usize, away: usize, seed: u32, half_ticks: u32) -> Setup {
    let side = |c: usize| SideSetup {
        club: c as u8,
        tactics: CLUBS[c].tactics,
        players: core::array::from_fn(|k| {
            let p = team::generate_player(c, k);
            ((c * 16 + k) as u16, p.number, p.attrs)
        }),
        morale: 50,
        bench: core::array::from_fn(|b| {
            let p = team::generate_player(c, 11 + b);
            (
                (c * 16 + 11 + b) as u16,
                p.number,
                p.attrs,
                p.pos == team::Pos::Gk,
            )
        }),
        bench_n: 5,
    };
    Setup {
        sides: [side(home), side(away)],
        half_ticks,
        seed,
        wet: false,
        humans: [false, false],
        keeper_ctl: [false, false],
        difficulty: 1,
    }
}

/// Play to full time with no human input; returns every event.
pub fn play(m: &mut Match, max: u32) -> Vec<Event> {
    let mut out = Vec::new();
    let mut seen = m.event_seq;
    let mut n = 0;
    while !m.finished() && n < max {
        m.step([Pad::default(); 2]);
        n += 1;
        for (_, e) in m.events_since(seen) {
            out.push(e);
        }
        seen = m.event_seq;
    }
    out
}

/// Aggregate numbers over a set of AI-vs-AI matches.
#[derive(Default, Debug)]
pub struct Metrics {
    pub matches: u32,
    pub goals: u32,
    pub shots: u32,
    pub on_target: u32,
    pub saves: u32,
    pub posts: u32,
    pub offsides: u32,
    pub fouls: u32,
    pub yellows: u32,
    pub reds: u32,
    pub corners: u32,
    pub throws: u32,
    pub goal_kicks: u32,
    pub free_kicks: u32,
    pub penalties: u32,
    pub tackles: u32,
    pub passes: u32,
    pub completed: u32,
    pub owned: u32,
    pub play: u32,
    pub diag: Diag,
}

pub fn metrics(pairs: &[(usize, usize, u32)], half_ticks: u32) -> Metrics {
    let mut out = Metrics::default();
    for &(h, a, seed) in pairs {
        let mut m = Match::new(&setup(h, a, seed, half_ticks));
        let ev = play(&mut m, half_ticks * 4);
        assert!(m.finished(), "{h} v {a} seed {seed}: never finished");
        let count =
            |f: &dyn Fn(&EventKind) -> bool| ev.iter().filter(|e| f(&e.kind)).count() as u32;
        out.matches += 1;
        out.goals += count(&|k| matches!(k, EventKind::Goal { .. }));
        out.shots += m.stats[0].shots as u32 + m.stats[1].shots as u32;
        out.on_target += m.stats[0].on_target as u32 + m.stats[1].on_target as u32;
        out.saves += count(&|k| *k == EventKind::Save);
        out.posts += count(&|k| *k == EventKind::Post);
        out.offsides += count(&|k| *k == EventKind::Offside);
        out.fouls += count(&|k| *k == EventKind::Foul);
        out.yellows += count(&|k| *k == EventKind::Yellow);
        out.reds += count(&|k| *k == EventKind::Red);
        out.corners += count(&|k| *k == EventKind::Corner);
        out.throws += count(&|k| *k == EventKind::ThrowIn);
        out.goal_kicks += count(&|k| *k == EventKind::GoalKick);
        out.free_kicks += count(&|k| *k == EventKind::FreeKick);
        out.penalties += count(&|k| *k == EventKind::Penalty);
        out.tackles += count(&|k| *k == EventKind::Tackle);
        for s in 0..2 {
            out.passes += m.stats[s].passes as u32;
            out.completed += m.stats[s].completed as u32;
            out.owned += m.stats[s].possession;
        }
        out.play += half_ticks * 2;
        let d = m.diag;
        out.diag.ground += d.ground;
        out.diag.lofted += d.lofted;
        out.diag.to_mate += d.to_mate;
        out.diag.to_opp += d.to_opp;
        out.diag.dead += d.dead;
        out.diag.intended += d.intended;
        out.diag.off_kicks += d.off_kicks;
        out.diag.off_aimed += d.off_aimed;
        out.diag.ground_mate += d.ground_mate;
        out.diag.ground_opp += d.ground_opp;
        out.diag.won += d.won;
        out.diag.won_high += d.won_high;
        out.diag.rushes += d.rushes;
        out.diag.smothers += d.smothers;
        out.diag.tried += d.tried;
        for k in 0..7 {
            out.diag.chose[k] += d.chose[k];
        }
    }
    out
}

/// The 10-match sample the tuning targets (every club plays).
pub const SAMPLE: [(usize, usize, u32); 10] = [
    (0, 1, 7),
    (2, 3, 11),
    (4, 5, 13),
    (6, 7, 17),
    (8, 9, 19),
    (1, 6, 23),
    (3, 8, 29),
    (5, 0, 31),
    (7, 2, 37),
    (9, 4, 41),
];

#[test]
#[ignore = "tuning aid: prints per-match averages"]
fn print_metrics() {
    let m = metrics(&SAMPLE, 60 * 60 * 3);
    let n = m.matches as f32;
    std::println!(
        "per match: goals {:.1} shots {:.1} on target {:.1} saves {:.1} posts {:.1} offsides {:.1} fouls {:.1} yellows {:.1} reds {:.2} corners {:.1} throws {:.1} goal kicks {:.1} free kicks {:.1} penalties {:.2} tackles {:.1} passes {:.0} completion {:.0}% owned {:.0}%",
        m.goals as f32 / n,
        m.shots as f32 / n,
        m.on_target as f32 / n,
        m.saves as f32 / n,
        m.posts as f32 / n,
        m.offsides as f32 / n,
        m.fouls as f32 / n,
        m.yellows as f32 / n,
        m.reds as f32 / n,
        m.corners as f32 / n,
        m.throws as f32 / n,
        m.goal_kicks as f32 / n,
        m.free_kicks as f32 / n,
        m.penalties as f32 / n,
        m.tackles as f32 / n,
        m.passes as f32 / n,
        100.0 * m.completed as f32 / m.passes.max(1) as f32,
        100.0 * m.owned as f32 / m.play as f32
    );
    std::println!("passes: {:?}", m.diag);
}

/// Dump a match trace for `tools/solid_eleven/trace_view.py` (top-down
/// frames): `SE_TRACE=path cargo test -p solid-eleven trace_match -- --ignored`.
#[test]
#[ignore = "debug aid: writes a trace file"]
fn trace_match() {
    use std::io::Write;
    let path = std::env::var("SE_TRACE").unwrap_or_else(|_| "target/se_trace.csv".into());
    let seed: u32 = std::env::var("SE_SEED")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(7);
    let every: u32 = std::env::var("SE_EVERY")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(15);
    let mut m = Match::new(&setup(0, 1, seed, 60 * 60 * 3));
    let mut f = std::fs::File::create(&path).unwrap();
    let mut seen = 0;
    while !m.finished() && m.tick < 60 * 60 * 10 {
        m.step([Pad::default(); 2]);
        let mut evs = std::string::String::new();
        for (_, e) in m.events_since(seen) {
            evs.push_str(&std::format!("{:?}/{}/{} ", e.kind, e.side, e.player));
        }
        seen = m.event_seq;
        if !m.tick.is_multiple_of(every) && evs.is_empty() {
            continue;
        }
        let mut line = std::format!(
            "{},{:?},{},{},{:.2},{:.2},{:.2},{}",
            m.tick,
            m.phase,
            m.score[0],
            m.score[1],
            m.ball.pos.x.to_f32(),
            m.ball.pos.z.to_f32(),
            m.ball.pos.y.to_f32(),
            m.owner.map_or(-1, |o| o as i32)
        );
        for p in &m.players {
            line.push_str(&std::format!(
                ",{:.2},{:.2},{}",
                p.pos.x.to_f32(),
                p.pos.y.to_f32(),
                match p.act {
                    Act::Free => 'f',
                    Act::Kick { .. } => 'k',
                    Act::Tackle { .. } => 't',
                    Act::Slide { .. } => 's',
                    Act::Down { .. } => 'd',
                    Act::Dive { .. } => 'v',
                    Act::Hold { .. } => 'h',
                    Act::Celebrate => 'c',
                }
            ));
        }
        line.push_str(&std::format!(",\"{}\"", evs.trim()));
        writeln!(f, "{line}").unwrap();
    }
}

/// The AI-vs-AI sample plays like football: every match ends, the score
/// is the goals logged, every restart happens somewhere in the sample,
/// players tire, and the numbers stay in football's range (per 6-minute
/// match). A regression guard for the AI's tuning.
#[test]
fn ai_matches_play_like_football() {
    let mut kinds = [0u32; 17];
    let mut m_sum = Metrics::default();
    for &(h, a, seed) in &SAMPLE {
        let mut m = Match::new(&setup(h, a, seed, 60 * 60 * 3));
        let ev = play(&mut m, 60 * 60 * 12);
        assert!(m.finished(), "{h} v {a}: never finished");
        // The score is the goals logged, by side.
        for s in 0..2 {
            let goals = ev
                .iter()
                .filter(|e| matches!(e.kind, EventKind::Goal { .. }) && e.side as usize == s)
                .count();
            assert_eq!(m.score[s] as usize, goals, "{h} v {a} side {s}");
            assert!(
                m.stats[s].on_target as usize >= goals.saturating_sub(1),
                "{h} v {a}"
            );
        }
        for e in &ev {
            kinds[event_index(e.kind)] += 1;
        }
        // Everyone stayed in the stadium; outfield players tired.
        let mut stamina = 0.0;
        let mut n = 0.0;
        for p in &m.players {
            assert!(
                p.pos.x.abs() < super::pitch::HALF_LEN + super::pitch::RUNOFF + super::fx::Fx::ONE
            );
            assert!(
                p.pos.y.abs()
                    < super::pitch::HALF_WID + super::pitch::RUNOFF + super::fx::Fx::int(7)
            );
            if p.role != super::team::Role::Gk && !p.off {
                stamina += p.stamina.to_f32();
                n += 1.0;
            }
        }
        assert!(
            stamina / n < 0.97,
            "{h} v {a}: nobody tired ({})",
            stamina / n
        );
        let mut one = Metrics::default();
        for s in 0..2 {
            one.passes += m.stats[s].passes as u32;
            one.completed += m.stats[s].completed as u32;
            one.owned += m.stats[s].possession;
            one.shots += m.stats[s].shots as u32;
        }
        m_sum.passes += one.passes;
        m_sum.completed += one.completed;
        m_sum.owned += one.owned;
        m_sum.shots += one.shots;
        m_sum.goals += (m.score[0] + m.score[1]) as u32;
        m_sum.play += 60 * 60 * 6;
        m_sum.matches += 1;
    }
    for (k, name) in [
        (EventKind::KickOff, "kick-off"),
        (EventKind::Goal { own_goal: false }, "goal"),
        (EventKind::Shot, "shot"),
        (EventKind::Save, "save"),
        (EventKind::Corner, "corner"),
        (EventKind::ThrowIn, "throw-in"),
        (EventKind::GoalKick, "goal kick"),
        (EventKind::FreeKick, "free kick"),
        (EventKind::Foul, "foul"),
        (EventKind::Yellow, "yellow card"),
        (EventKind::HalfTime, "half time"),
        (EventKind::FullTime, "full time"),
        (EventKind::Tackle, "tackle"),
    ] {
        assert!(
            kinds[event_index(k)] > 0,
            "no {name} in the sample: {kinds:?}"
        );
    }
    let n = m_sum.matches as f32;
    let goals = m_sum.goals as f32 / n;
    let shots = m_sum.shots as f32 / n;
    let completion = m_sum.completed as f32 / m_sum.passes as f32;
    let owned = m_sum.owned as f32 / m_sum.play as f32;
    assert!((0.8..4.5).contains(&goals), "goals per match {goals}");
    assert!((4.0..16.0).contains(&shots), "shots per match {shots}");
    assert!(
        (0.55..0.88).contains(&completion),
        "pass completion {completion}"
    );
    assert!((0.35..0.7).contains(&owned), "ball owned {owned}");
}

fn event_index(k: EventKind) -> usize {
    match k {
        EventKind::KickOff => 0,
        EventKind::Goal { .. } => 1,
        EventKind::Shot => 2,
        EventKind::Save => 3,
        EventKind::Post => 4,
        EventKind::Offside => 5,
        EventKind::Foul => 6,
        EventKind::Yellow => 7,
        EventKind::Red => 8,
        EventKind::Corner => 9,
        EventKind::ThrowIn => 10,
        EventKind::GoalKick => 11,
        EventKind::FreeKick => 12,
        EventKind::Penalty => 13,
        EventKind::HalfTime => 14,
        EventKind::FullTime => 15,
        EventKind::Tackle => 16,
        EventKind::Substitution => 0,
    }
}

/// The same pads give the same match: twice the same full match, bit for
/// bit (the cross-target check rides on this and the self-test's hash).
#[test]
fn a_full_match_is_deterministic() {
    let run = || {
        let mut m = Match::new(&setup(3, 6, 99, 60 * 60));
        let mut h = super::fx::Hasher::new();
        let mut n = 0;
        while !m.finished() && n < 60 * 60 * 5 {
            let mut pads = [Pad::default(); 2];
            // A human on side 0, pressing things on a schedule.
            pads[0].mv = super::fx::V2::new(super::fx::Fx::ONE, super::fx::Fx::ZERO);
            pads[0].sprint = n % 200 < 120;
            if n % 97 == 0 {
                pads[0].pass = Some(super::fx::Fx::HALF);
            }
            if n % 151 == 0 {
                pads[0].shoot = Some(super::fx::Fx::milli(700));
            }
            if n % 61 == 0 {
                pads[0].switch = true;
            }
            m.step(pads);
            if n % 60 == 0 {
                h.word(m.hash());
            }
            n += 1;
        }
        (h.0, m.score, m.tick)
    };
    assert_eq!(run(), run());
}

/// Substitutions: the AI changes tired players after half an hour, at
/// most three, never during play; a human's request goes through at the
/// next dead ball; a keeper is only replaced by a keeper.
#[test]
fn substitutions_follow_the_rules() {
    let mut m = Match::new(&setup(0, 1, 5, 60 * 60 * 2));
    let mut subs_seen = [0u32; 2];
    let mut n = 0;
    let mut seen = 0;
    while !m.finished() && n < 60 * 60 * 8 {
        // Side 0 is AI here; force fatigue on one player to trigger it.
        if n == 60 * 60 * 2 + 600 {
            m.players[5].stamina = super::fx::Fx::milli(200);
        }
        let before = m.phase;
        m.step([Pad::default(); 2]);
        for (_, e) in m.events_since(seen) {
            if e.kind == EventKind::Substitution {
                subs_seen[e.side as usize] += 1;
                // Only at a dead ball: the phase before was not open play
                // (a restart begins from Dead/Goal/HalfTime).
                assert_ne!(before, Phase::Play, "substitution during play");
                assert_ne!(
                    e.player as usize % SIDE_N,
                    0,
                    "no keeper change without a red card"
                );
            }
        }
        seen = m.event_seq;
        n += 1;
    }
    assert!(
        subs_seen[0] >= 1,
        "the tired player was replaced: {subs_seen:?}"
    );
    assert!(subs_seen.iter().all(|&k| k <= MAX_SUBS as u32));
    assert_eq!(m.subs[0] as u32, subs_seen[0]);
    let p = &m.players[5];
    assert!(p.db != 5, "a new man in slot 5");

    // A human request.
    let mut s = setup(2, 3, 9, 60 * 60);
    s.humans = [true, false];
    let mut m = Match::new(&s);
    let pad = Pad {
        sub: true,
        ..Pad::default()
    };
    let mut subbed = false;
    for t in 0..60 * 60 * 2 {
        let p = if t == 120 { pad } else { Pad::default() };
        m.step([p, Pad::default()]);
        if m.subs[0] == 1 {
            subbed = true;
            break;
        }
    }
    assert!(subbed, "the human's substitution happened at a dead ball");
    assert_eq!(m.bench_n[0], 4);
}

const SIDE_N: usize = 11;

// ------------------------------------------------------------ keepers

/// A match in play with side 0 human and allowed (or not) to take its
/// keeper; the ball loose by the touchline at halfway, out of everyone's
/// reach (a touch would hand control to whoever plays it).
fn keeper_match(allowed: bool) -> Match {
    let mut st = setup(0, 1, 11, 3_600);
    st.humans = [true, false];
    st.keeper_ctl = [allowed, false];
    let mut m = Match::new(&st);
    m.phase = Phase::Play;
    m.owner = None;
    m.ball = super::ball::Ball::at_rest(V2::new(Fx::ZERO, Fx::int(32)));
    m
}

fn press(m: &mut Match, pad: Pad) {
    m.step([pad, Pad::default()]);
}

#[test]
fn the_keeper_button_hands_over_the_keeper_only_when_allowed() {
    for allowed in [false, true] {
        let mut m = keeper_match(allowed);
        let k = (0..SIDE)
            .find(|&i| m.players[i].role == team::Role::Gk)
            .unwrap();
        press(
            &mut m,
            Pad {
                keeper: true,
                ..Pad::default()
            },
        );
        assert_eq!(m.active[0] as usize == k, allowed, "allowed = {allowed}");
        // Switch goes back to the field.
        press(
            &mut m,
            Pad {
                switch: true,
                ..Pad::default()
            },
        );
        press(&mut m, Pad::default());
        assert_ne!(m.active[0] as usize, k, "switch leaves the keeper");
    }
}

#[test]
fn a_shot_at_his_goal_gives_the_human_the_keeper() {
    let mut m = keeper_match(true);
    let k = m.keeper_of(0).unwrap();
    m.set_shot(Some(1));
    press(&mut m, Pad::default());
    assert_eq!(m.active[0] as usize, k);
    // Switch away for the rebound: the shot is still live, but it handed
    // the keeper over once and doesn't take him back.
    press(
        &mut m,
        Pad {
            switch: true,
            ..Pad::default()
        },
    );
    for _ in 0..30 {
        press(&mut m, Pad::default());
        assert!(m.shot().is_some());
        assert_ne!(m.active[0] as usize, k, "the keeper came back");
    }
    // Not when control of the keeper is off.
    let mut m = keeper_match(false);
    m.set_shot(Some(1));
    press(&mut m, Pad::default());
    assert_ne!(m.active[0] as usize, m.keeper_of(0).unwrap());
}

#[test]
fn a_human_keeper_dives_where_the_stick_points() {
    let mut m = keeper_match(true);
    let k = m.keeper_of(0).unwrap();
    press(
        &mut m,
        Pad {
            keeper: true,
            ..Pad::default()
        },
    );
    let before = m.players[k].pos;
    press(
        &mut m,
        Pad {
            tackle: true,
            mv: V2::new(Fx::ZERO, Fx::ONE),
            ..Pad::default()
        },
    );
    assert!(
        matches!(m.players[k].act, Act::Dive { .. }),
        "{:?}",
        m.players[k].act
    );
    for _ in 0..10 {
        press(&mut m, Pad::default());
    }
    assert!(
        m.players[k].pos.y > before.y + Fx::milli(500),
        "dove along +y"
    );
}

#[test]
fn a_human_keeper_throws_or_punts_from_his_hands() {
    for (pad, long) in [
        (
            Pad {
                pass: Some(Fx::milli(300)),
                ..Pad::default()
            },
            false,
        ),
        (
            Pad {
                shoot: Some(Fx::ONE),
                ..Pad::default()
            },
            true,
        ),
    ] {
        let mut m = keeper_match(true);
        let k = m.keeper_of(0).unwrap();
        // He holds the ball in his area.
        m.players[k].pos =
            m.ends[0].goal_center() + V2::new(m.ends[0].sign().mul(Fx::int(-6)), Fx::ZERO);
        m.ball = super::ball::Ball::at_rest(m.players[k].pos);
        m.owner = Some(k as u8);
        m.players[k].act = Act::Hold { t: HUMAN_HOLD };
        press(&mut m, Pad::default());
        assert_eq!(m.active[0] as usize, k, "holding: the human has him");
        let mv = V2::new(m.ends[0].sign().mul(Fx::int(-1)), Fx::ZERO);
        press(&mut m, Pad { mv, ..pad });
        let mut aimed = None;
        for _ in 0..20 {
            press(&mut m, Pad::default());
            if aimed.is_none() && m.owner != Some(k as u8) {
                aimed = Some(m.receiver);
            }
        }
        assert_ne!(m.owner, Some(k as u8), "the ball left his hands");
        if !long {
            // A throw goes to a teammate, who is told to come for it.
            let r = aimed.flatten().expect("the throw has a receiver");
            assert!(side_of(r as usize) == 0 && r as usize != k);
        }
        let gone = (m.ball.pos.ground() - m.players[k].pos).len();
        assert!(gone > Fx::int(3), "it travelled ({gone:?})");
        let up = m.ball.vel.y.abs() + m.ball.pos.y;
        assert_eq!(up > Fx::int(2), long, "a punt goes up, a throw stays low");
    }
}

#[test]
fn a_human_keeper_holds_longer_but_not_forever() {
    let mut m = keeper_match(true);
    let k = m.keeper_of(0).unwrap();
    m.players[k].pos =
        m.ends[0].goal_center() + V2::new(m.ends[0].sign().mul(Fx::int(-5)), Fx::ZERO);
    m.ball = super::ball::Ball::at_rest(m.players[k].pos);
    m.owner = Some(k as u8);
    m.players[k].act = Act::Hold { t: HUMAN_HOLD };
    let mut released = None;
    for t in 0..(HUMAN_HOLD as u32 + 60) {
        press(&mut m, Pad::default());
        if m.owner != Some(k as u8) {
            released = Some(t);
            break;
        }
    }
    let t = released.expect("the ball is played for him in the end");
    assert!(t + 20 >= HUMAN_HOLD as u32, "not before his time ({t})");
}

#[test]
fn a_keeper_out_of_his_area_has_no_hands() {
    let mut m = keeper_match(true);
    let k = m.keeper_of(0).unwrap();
    let out = m.ends[0].goal_center() + V2::new(m.ends[0].sign().mul(Fx::int(-25)), Fx::ZERO);
    m.players[k].pos = out;
    assert!(!super::pitch::in_penalty_area(out, m.ends[0]));
    for p in m.players.iter_mut().filter(|p| p.role != team::Role::Gk) {
        p.pos = V2::new(Fx::int(40), Fx::int(30));
    }
    m.ball = super::ball::Ball::at_rest(out);
    press(
        &mut m,
        Pad {
            keeper: true,
            ..Pad::default()
        },
    );
    for _ in 0..5 {
        press(&mut m, Pad::default());
    }
    assert!(
        !matches!(m.players[k].act, Act::Hold { .. }),
        "no catch outside the box"
    );
}

#[test]
fn a_sent_off_keeper_leaves_the_goal_kick_to_a_teammate() {
    let mut m = Match::new(&setup(0, 1, 5, 3_600));
    let k = m.keeper_of(0).unwrap();
    m.players[k].off = true;
    let spot = V2::new(Fx::int(-46), Fx::int(5));
    m.begin_restart(super::rules::Restart {
        kind: super::rules::RestartKind::GoalKick,
        team: 0,
        spot,
    });
    let taker = m.taker as usize;
    assert!(!m.players[taker].off, "taker {taker} is off the pitch");
    assert!(taker / SIDE == 0);
    // And it's taken: the ball leaves the spot.
    for _ in 0..900 {
        m.step([Pad::default(); 2]);
        if !(m.ball.pos.ground() - spot).shorter_than(Fx::int(3)) {
            return;
        }
    }
    panic!("the goal kick was never taken");
}

#[test]
fn manual_substitution_swaps_player_attributes_and_stamina() {
    let mut m = Match::new(&setup(0, 1, 5, 3_600));
    let orig_db = m.players[1].db;
    let bench_db = m.bench[0][1].0;
    let bench_before = m.bench_n[0];
    assert!(m.substitute_player(0, 1, 1));
    assert_eq!(m.players[1].db, bench_db);
    assert_ne!(m.players[1].db, orig_db);
    assert_eq!(m.players[1].stamina, Fx::ONE);
    assert_eq!(m.bench_n[0], bench_before - 1);
    assert_eq!(m.subs[0], 1);
}

#[test]
fn start_second_half_transitions_smoothly() {
    let mut m = Match::new(&setup(0, 1, 5, 3_600));
    m.start_second_half();
    assert_eq!(m.half, 2);
    assert_eq!(m.clock, 0);
    assert_eq!(m.players[0].vel, V2::ZERO);
}

/// Tuning aid: ticks per phase over the sample (the dead time restarts
/// cost now that everyone walks into place), and the longest walk-in.
/// `cargo test -p solid-eleven --release print_phase_ticks -- --ignored --nocapture`
#[test]
#[ignore = "tuning aid"]
fn print_phase_ticks() {
    for &(h, a, seed) in &SAMPLE {
        let mut m = Match::new(&setup(h, a, seed, 60 * 60 * 3));
        let mut per = [0u32; 7];
        let mut n = 0;
        let mut longest = (0u16, super::rules::RestartKind::KickOff);
        let mut longest_ready = (0u16, super::rules::RestartKind::KickOff);
        while !m.finished() && n < 60 * 60 * 30 {
            let ph = match m.phase {
                Phase::Setup { t } => {
                    if t > longest.0 {
                        longest = (t, m.restart.kind);
                    }
                    0
                }
                Phase::Ready { t } => {
                    if t > longest_ready.0 {
                        longest_ready = (t, m.restart.kind);
                    }
                    1
                }
                Phase::Play => 2,
                Phase::Dead { .. } => 3,
                Phase::Goal { .. } => 4,
                Phase::HalfTime { .. } => 5,
                Phase::FullTime => 6,
            };
            per[ph] += 1;
            m.step([Pad::default(); 2]);
            n += 1;
        }
        std::println!(
            "{h}v{a}: per phase {per:?} total {n} longest setup {longest:?} ready {longest_ready:?}"
        );
    }
}

/// Tuning aid: the sample's numbers over 60 matches, in parallel.
/// `cargo test -p solid-eleven --release print_big_metrics -- --ignored --nocapture`
#[test]
#[ignore = "tuning aid"]
fn print_big_metrics() {
    let mut pairs = Vec::new();
    for k in 0..60u32 {
        let h = (k % 10) as usize;
        let a = ((k / 10 + 1 + k % 10) % 10) as usize;
        let a = if a == h { (a + 1) % 10 } else { a };
        pairs.push((h, a, k * 13 + 5));
    }
    let out = std::sync::Mutex::new(Vec::new());
    std::thread::scope(|sc| {
        for chunk in pairs.chunks(5) {
            let out = &out;
            sc.spawn(move || {
                let m = metrics(chunk, 60 * 60 * 3);
                out.lock().unwrap().push(m);
            });
        }
    });
    let all = out.into_inner().unwrap();
    let sum = |f: &dyn Fn(&Metrics) -> u32| all.iter().map(f).sum::<u32>() as f32;
    let n = sum(&|m| m.matches);
    std::println!(
        "{n} matches: goals {:.2} shots {:.1} fouls {:.1} yellows {:.2} reds {:.2} pens {:.2} tackles {:.1} won {:.1} high {:.1} tried {:.1} rushes {:.2} smothers {:.2} completion {:.1}% owned {:.1}%",
        sum(&|m| m.goals) / n,
        sum(&|m| m.shots) / n,
        sum(&|m| m.fouls) / n,
        sum(&|m| m.yellows) / n,
        sum(&|m| m.reds) / n,
        sum(&|m| m.penalties) / n,
        sum(&|m| m.tackles) / n,
        sum(&|m| m.diag.won) / n,
        sum(&|m| m.diag.won_high) / n,
        sum(&|m| m.diag.tried) / n,
        sum(&|m| m.diag.rushes) / n,
        sum(&|m| m.diag.smothers) / n,
        100.0 * sum(&|m| m.completed) / sum(&|m| m.passes),
        100.0 * sum(&|m| m.owned) / sum(&|m| m.play),
    );
}

/// Issue #86: nobody is teleported between the whistle and the restart
/// (after goals, throw-ins, free kicks, corners, goal kicks, cards): every
/// move is a step on his legs, a sprint at most. And the taker walks round
/// the ball to line up his kick: at the kick he faces where it goes.
#[test]
fn restarts_never_teleport_anyone_and_takers_line_up() {
    let mut restarts = 0;
    let mut lined_up = 0;
    let mut walked_round = 0;
    for &(h, a, seed) in &SAMPLE[..3] {
        let mut m = Match::new(&setup(h, a, seed, 60 * 60 * 3));
        let mut ready_from: Option<V2> = None;
        let mut n = 0;
        while !m.finished() && n < 60 * 60 * 12 {
            let before: [V2; N] = core::array::from_fn(|i| m.players[i].pos);
            let off: [bool; N] = core::array::from_fn(|i| m.players[i].off);
            let phase = m.phase;
            m.step([Pad::default(); 2]);
            n += 1;
            if matches!(phase, Phase::Ready { t: 0 }) {
                ready_from = Some(before[m.taker as usize]);
            }
            // Walking into place: a sprint's step at most. While the ball
            // is dead or a goal is cheered, bodies still shove each other
            // apart (`separate`), as in play.
            let max = match phase {
                Phase::Setup { .. } | Phase::Ready { .. } => Fx::milli(165),
                Phase::Dead { .. } | Phase::Goal { .. } => Fx::milli(300),
                _ => Fx::int(999),
            };
            {
                for i in 0..N {
                    if off[i] || m.players[i].off {
                        continue;
                    }
                    let d = (m.players[i].pos - before[i]).len();
                    assert!(
                        d <= max,
                        "{h} v {a}: player {i} jumped {:.3} m in {phase:?} (tick {})",
                        d.to_f32(),
                        m.tick
                    );
                }
            }
            if matches!(phase, Phase::Ready { .. }) && m.phase == Phase::Play {
                let k = m.taker as usize;
                if let Act::Kick { plan, .. } = m.players[k].act {
                    restarts += 1;
                    let dir = plan.vel.ground().norm();
                    if m.players[k].face.dot(dir) > Fx::milli(980) {
                        lined_up += 1;
                    }
                    if let Some(p) = ready_from
                        && !(m.players[k].pos - p).shorter_than(Fx::milli(300))
                    {
                        walked_round += 1;
                    }
                }
            }
        }
        assert!(m.finished());
    }
    assert!(restarts > 30, "{restarts} restarts");
    assert!(
        lined_up * 10 >= restarts * 9,
        "{lined_up} of {restarts} lined up"
    );
    assert!(
        walked_round * 4 >= restarts,
        "{walked_round} of {restarts} walked round the ball"
    );
}

/// One side pressing with Normal's urgency against the same club with
/// none (the old pressing), sides swapped every other match, `n` matches
/// in parallel. Per side (urgent, other): possessions won, goals, fouls,
/// possessions won in the winner's attacking half, ticks on the ball;
/// then the other side's passes completed and tried.
fn press_duel(n: u32) -> [u32; 12] {
    let mut jobs = Vec::new();
    for k in 0..n {
        let h = (k % 10) as usize;
        let a = ((k / 10 + 1 + k % 10) % 10) as usize;
        let a = if a == h { (a + 1) % 10 } else { a };
        jobs.push((h, a, k * 13 + 5, (k % 2) as usize));
    }
    let out = std::sync::Mutex::new([0u32; 12]);
    std::thread::scope(|sc| {
        for chunk in jobs.chunks(5) {
            let out = &out;
            sc.spawn(move || {
                for &(h, a, seed, urgent) in chunk {
                    let mut m = Match::new(&setup(h, a, seed, 60 * 60 * 3));
                    let mut u = [Fx::ZERO; 2];
                    u[urgent] = Fx::ONE;
                    m.urgency_test = Some(u);
                    play(&mut m, 60 * 60 * 12);
                    assert!(m.finished());
                    let o = 1 - urgent;
                    let mut t = out.lock().unwrap();
                    t[0] += m.diag.won_by[urgent];
                    t[1] += m.diag.won_by[o];
                    t[2] += m.score[urgent] as u32;
                    t[3] += m.score[o] as u32;
                    t[4] += m.stats[urgent].fouls as u32;
                    t[5] += m.stats[o].fouls as u32;
                    t[6] += m.diag.won_high_by[urgent];
                    t[7] += m.diag.won_high_by[o];
                    t[8] += m.stats[urgent].possession;
                    t[9] += m.stats[o].possession;
                    t[10] += m.stats[o].completed as u32;
                    t[11] += m.stats[o].passes as u32;
                }
            });
        }
    });
    out.into_inner().unwrap()
}

/// Issue #86: Normal's pressing hunts the ball higher and harder than
/// the old pressing, within the laws. Against the same clubs pressing the
/// old way it wins the ball back in the opponents' half more often, keeps
/// more of the ball, and commits no more fouls (it tackles from the
/// front, not from behind).
#[test]
fn normal_pressing_wins_the_ball_back_higher_within_the_laws() {
    let o = press_duel(30);
    std::println!("press duel: {o:?}");
    assert!(
        o[6] * 100 > o[7] * 115,
        "high recoveries {} vs {}",
        o[6],
        o[7]
    );
    let poss = o[8] as f32 / (o[8] + o[9]) as f32;
    assert!(poss > 0.52, "possession {poss:.3}");
    assert!(o[4] <= o[5], "fouls {} vs {}", o[4], o[5]);
}

/// Tuning aid: [`press_duel`] over 60 matches, printed.
#[test]
#[ignore = "tuning aid"]
fn print_press_duel() {
    let o = press_duel(60);
    std::println!(
        "urgent side: won {} vs {} ({:.1}%), goals {} vs {}, fouls {} vs {}, high {} vs {}, possession {:.1}%, the other's completion {:.1}%",
        o[0],
        o[1],
        100.0 * o[0] as f32 / (o[0] + o[1]) as f32,
        o[2],
        o[3],
        o[4],
        o[5],
        o[6],
        o[7],
        100.0 * o[8] as f32 / (o[8] + o[9]) as f32,
        100.0 * o[10] as f32 / o[11] as f32
    );
}
