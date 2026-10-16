//! Whole-match tests: invariants of AI-vs-AI matches, determinism, and
//! the rules as they happen in play.

extern crate std;
use std::vec::Vec;

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
