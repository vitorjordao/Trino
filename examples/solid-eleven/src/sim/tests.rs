//! Whole-match tests: invariants of AI-vs-AI matches, determinism, and
//! the rules as they happen in play.

extern crate std;
use std::vec::Vec;

use super::engine::MAX_SUBS;
use super::engine::*;
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
        out.diag.ground_mate += d.ground_mate;
        out.diag.ground_opp += d.ground_opp;
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
