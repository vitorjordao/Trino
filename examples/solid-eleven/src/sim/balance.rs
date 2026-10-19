//! Balance: the game is fair and every lever does what it says. Each test
//! simulates many full AI-vs-AI matches (or seasons) and prints its table
//! (`cargo test -p solid-eleven balance -- --nocapture`), then holds the
//! numbers to ranges:
//!
//! - every club can win and none runs away with the league (equal point
//!   budgets, different styles);
//! - neither end nor the first kick-off favors a side;
//! - better attributes win more (the editor's points matter), and the
//!   difficulty setting sharpens the AI;
//! - the economy keeps every club afloat and gives no free money.

extern crate std;
use std::vec::Vec;

use super::engine::*;
use super::league::{Career, ROUNDS};
use super::team::{self, ATTRS, CLUBS, TEAMS, Tactics};
use super::tests::{play, setup};

/// Ticks per half: 3 minutes of play (the default length).
const HALF: u32 = 60 * 60 * 3;

/// Play `jobs` on every core; results in job order.
fn par<J: Sync, R: Send + Default + Clone>(jobs: &[J], f: impl Fn(&J) -> R + Sync) -> Vec<R> {
    let n = std::thread::available_parallelism()
        .map_or(4, |n| n.get())
        .min(16);
    let out = std::sync::Mutex::new(vec![R::default(); jobs.len()]);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..n {
            s.spawn(|| {
                loop {
                    let i = next.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if i >= jobs.len() {
                        break;
                    }
                    let r = f(&jobs[i]);
                    out.lock().unwrap()[i] = r;
                }
            });
        }
    });
    out.into_inner().unwrap()
}

/// A full match from `st`: the final score.
fn result(st: &Setup) -> [u8; 2] {
    let mut m = Match::new(st);
    play(&mut m, st.half_ticks * 4);
    assert!(m.finished());
    m.score
}

#[derive(Clone, Copy, Default, Debug)]
struct Rec {
    w: u32,
    d: u32,
    l: u32,
    gf: u32,
    ga: u32,
}

impl Rec {
    fn add(&mut self, gf: u8, ga: u8) {
        self.gf += gf as u32;
        self.ga += ga as u32;
        match gf.cmp(&ga) {
            core::cmp::Ordering::Greater => self.w += 1,
            core::cmp::Ordering::Equal => self.d += 1,
            core::cmp::Ordering::Less => self.l += 1,
        }
    }
    fn played(&self) -> u32 {
        self.w + self.d + self.l
    }
    fn ppg(&self) -> f32 {
        (self.w * 3 + self.d) as f32 / self.played().max(1) as f32
    }
}

/// Every club home and away against every other, four times (seeds).
fn round_robin() -> ([Rec; TEAMS], Rec) {
    let mut jobs = Vec::new();
    for seed in [101u32, 202, 303, 404] {
        for h in 0..TEAMS {
            for a in 0..TEAMS {
                if h != a {
                    jobs.push((h, a, seed ^ (h as u32 * 31 + a as u32 * 7)));
                }
            }
        }
    }
    let scores = par(&jobs, |&(h, a, seed)| result(&setup(h, a, seed, HALF)));
    let mut clubs = [Rec::default(); TEAMS];
    let mut home = Rec::default();
    for (&(h, a, _), s) in jobs.iter().zip(&scores) {
        clubs[h].add(s[0], s[1]);
        clubs[a].add(s[1], s[0]);
        home.add(s[0], s[1]);
    }
    (clubs, home)
}

#[test]
fn every_club_can_win_and_none_runs_away_with_it() {
    let (clubs, home) = round_robin();
    std::println!("club                  P   W   D   L   GF  GA  PTS/GAME");
    let mut ppg = Vec::new();
    for (c, r) in clubs.iter().enumerate() {
        std::println!(
            "{:<20} {:>3} {:>3} {:>3} {:>3} {:>4} {:>3}  {:.2}",
            CLUBS[c].name,
            r.played(),
            r.w,
            r.d,
            r.l,
            r.gf,
            r.ga,
            r.ppg()
        );
        ppg.push(r.ppg());
    }
    let n = home.played() as f32;
    std::println!(
        "{} matches: home wins {:.0}%, draws {:.0}%, away wins {:.0}%, {:.2} goals a match",
        home.played(),
        home.w as f32 / n * 100.0,
        home.d as f32 / n * 100.0,
        home.l as f32 / n * 100.0,
        (home.gf + home.ga) as f32 / n
    );
    let (lo, hi) = ppg
        .iter()
        .fold((f32::MAX, f32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
    for (c, r) in clubs.iter().enumerate() {
        let p = r.played();
        assert!(r.w * 7 >= p, "{} won only {} of {p}", CLUBS[c].name, r.w);
        assert!(r.l * 7 >= p, "{} lost only {} of {p}", CLUBS[c].name, r.l);
    }
    assert!(lo >= 0.9 && hi <= 1.9, "points per game {lo:.2}..{hi:.2}");
    assert!(hi - lo <= 0.8, "spread {:.2}", hi - lo);
    let draws = home.d as f32 / n;
    assert!((0.15..0.4).contains(&draws), "draws {draws:.2}");
}

/// The same club on both sides, many seeds: whoever kicks off first,
/// whichever end, neither side wins more than chance allows.
#[test]
fn neither_side_is_favored() {
    let mut jobs = Vec::new();
    for c in 0..TEAMS {
        for seed in 0..10u32 {
            jobs.push((c, seed * 977 + c as u32));
        }
    }
    let res = par(&jobs, |&(c, seed)| result(&setup(c, c, seed, HALF)));
    let mut side = Rec::default();
    for s in &res {
        side.add(s[0], s[1]);
    }
    std::println!(
        "{} mirror matches: side 0 won {}, drew {}, side 1 won {} (goals {} - {})",
        side.played(),
        side.w,
        side.d,
        side.l,
        side.gf,
        side.ga
    );
    // Within about two and a half standard deviations of a coin flip.
    let decided = (side.w + side.l) as f32;
    let diff = side.w as f32 - side.l as f32;
    assert!(diff.abs() <= 2.6 * decided.sqrt(), "{side:?}");
    let goals = (side.gf + side.ga) as f32;
    let gdiff = side.gf as f32 - side.ga as f32;
    assert!(gdiff.abs() <= 2.6 * goals.sqrt(), "{side:?}");
}

/// Per-match averages over a set of fixtures (events and stats).
#[derive(Clone, Copy, Default, Debug)]
pub struct Sample {
    pub matches: u32,
    pub goals: u32,
    pub shots: u32,
    pub on_target: u32,
    pub saves: u32,
    pub corners: u32,
    pub offsides: u32,
    pub fouls: u32,
    pub yellows: u32,
    pub reds: u32,
    pub penalties: u32,
    pub draws: u32,
    pub home_wins: u32,
    pub away_wins: u32,
}

impl Sample {
    fn add(&mut self, o: &Sample) {
        self.matches += o.matches;
        self.goals += o.goals;
        self.shots += o.shots;
        self.on_target += o.on_target;
        self.saves += o.saves;
        self.corners += o.corners;
        self.offsides += o.offsides;
        self.fouls += o.fouls;
        self.yellows += o.yellows;
        self.reds += o.reds;
        self.penalties += o.penalties;
        self.draws += o.draws;
        self.home_wins += o.home_wins;
        self.away_wins += o.away_wins;
    }

    pub fn per(&self, x: u32) -> f32 {
        x as f32 / self.matches.max(1) as f32
    }

    fn print(&self, title: &str) {
        std::println!(
            "{title}: {} matches | goals {:.2} | shots {:.1} (on target {:.0}%) | saves {:.0}% | corners {:.1} | offsides {:.2} | fouls {:.1} | yellows {:.2} | reds {:.2} | penalties {:.2} | home {:.0}% draw {:.0}% away {:.0}%",
            self.matches,
            self.per(self.goals),
            self.per(self.shots),
            100.0 * self.on_target as f32 / self.shots.max(1) as f32,
            100.0 * self.saves as f32 / self.on_target.max(1) as f32,
            self.per(self.corners),
            self.per(self.offsides),
            self.per(self.fouls),
            self.per(self.yellows),
            self.per(self.reds),
            self.per(self.penalties),
            100.0 * self.per(self.home_wins),
            100.0 * self.per(self.draws),
            100.0 * self.per(self.away_wins),
        );
    }
}

/// One match's numbers.
fn sample_of(st: &Setup) -> Sample {
    let mut m = Match::new(st);
    let ev = play(&mut m, st.half_ticks * 4);
    assert!(m.finished());
    let count = |f: &dyn Fn(&EventKind) -> bool| ev.iter().filter(|e| f(&e.kind)).count() as u32;
    let [h, a] = m.score;
    Sample {
        matches: 1,
        goals: (h + a) as u32,
        shots: m.stats[0].shots as u32 + m.stats[1].shots as u32,
        on_target: m.stats[0].on_target as u32 + m.stats[1].on_target as u32,
        saves: count(&|k| *k == EventKind::Save),
        corners: count(&|k| *k == EventKind::Corner),
        offsides: count(&|k| *k == EventKind::Offside),
        fouls: count(&|k| *k == EventKind::Foul),
        yellows: count(&|k| *k == EventKind::Yellow),
        reds: count(&|k| *k == EventKind::Red),
        penalties: count(&|k| *k == EventKind::Penalty),
        draws: (h == a) as u32,
        home_wins: (h > a) as u32,
        away_wins: (h < a) as u32,
    }
}

/// Every pairing once per seed (home and away).
fn all_fixtures(seeds: &[u32]) -> Vec<(usize, usize, u32)> {
    let mut jobs = Vec::new();
    for &seed in seeds {
        for h in 0..TEAMS {
            for a in 0..TEAMS {
                if h != a {
                    jobs.push((h, a, seed ^ (h as u32 * 31 + a as u32 * 7)));
                }
            }
        }
    }
    jobs
}

/// The tuning table over a big sample (release: `cargo test -p
/// solid-eleven --release tuning_table -- --ignored --nocapture`).
#[test]
#[ignore = "tuning aid: a big sample, prints the table"]
fn tuning_table() {
    let jobs = all_fixtures(&[1, 2, 3, 4]);
    let all = par(&jobs, |&(h, a, seed)| sample_of(&setup(h, a, seed, HALF)));
    let mut s = Sample::default();
    for x in &all {
        s.add(x);
    }
    s.print("round robin x4");
}

/// Obsidian FC (club 5) against everyone, home and away, with its tactics
/// changed one lever at a time: which one makes it strong?
#[test]
#[ignore = "tuning aid"]
fn which_lever() {
    let base = CLUBS[5].tactics;
    let variants: [(&str, Tactics); 6] = [
        ("as shipped", base),
        (
            "4-4-2",
            Tactics {
                formation: 0,
                ..base
            },
        ),
        (
            "aggression 50",
            Tactics {
                aggression: 50,
                ..base
            },
        ),
        ("direct 40", Tactics { direct: 40, ..base }),
        ("tempo 45", Tactics { tempo: 45, ..base }),
        (
            "all average",
            Tactics {
                formation: 0,
                press: 50,
                line: 50,
                width: 60,
                tempo: 50,
                direct: 40,
                aggression: 50,
            },
        ),
    ];
    for (name, t) in variants {
        let mut jobs = Vec::new();
        for seed in [1u32, 2, 3] {
            for o in 0..TEAMS {
                if o != 5 {
                    jobs.push((5usize, o, seed ^ (o as u32 * 13)));
                    jobs.push((o, 5usize, seed ^ (o as u32 * 17)));
                }
            }
        }
        let res = par(&jobs, |&(h, a, seed)| {
            let mut st = setup(h, a, seed, HALF);
            let side = if h == 5 { 0 } else { 1 };
            st.sides[side].tactics = t;
            result(&st)
        });
        let mut r = Rec::default();
        for (&(h, _, _), s) in jobs.iter().zip(&res) {
            if h == 5 {
                r.add(s[0], s[1])
            } else {
                r.add(s[1], s[0])
            }
        }
        std::println!(
            "{name:<14} W {:>2} D {:>2} L {:>2}  GF {:>3} GA {:>3}  {:.2}",
            r.w,
            r.d,
            r.l,
            r.gf,
            r.ga,
            r.ppg()
        );
    }
}

/// The football in numbers: per match (two 3-minute halves), over every
/// pairing twice — scoring, the keepers, set pieces, discipline.
#[test]
fn the_football_numbers_are_in_range() {
    let jobs = all_fixtures(&[7, 8]);
    let all = par(&jobs, |&(h, a, seed)| sample_of(&setup(h, a, seed, HALF)));
    let mut s = Sample::default();
    for x in &all {
        s.add(x);
    }
    s.print("every pairing x2");
    let on_target = s.on_target as f32 / s.shots.max(1) as f32;
    let saves = s.saves as f32 / s.on_target.max(1) as f32;
    for (name, v, lo, hi) in [
        ("goals", s.per(s.goals), 2.0, 3.4),
        ("shots", s.per(s.shots), 8.0, 16.0),
        ("on target share", on_target, 0.5, 0.8),
        ("save share", saves, 0.55, 0.8),
        ("corners", s.per(s.corners), 0.5, 4.0),
        ("offsides", s.per(s.offsides), 0.1, 2.0),
        ("fouls", s.per(s.fouls), 1.5, 8.0),
        ("yellows", s.per(s.yellows), 0.2, 2.0),
        ("reds", s.per(s.reds), 0.0, 0.3),
        ("penalties", s.per(s.penalties), 0.05, 0.5),
    ] {
        assert!(
            (lo..=hi).contains(&v),
            "{name} per match {v:.2} outside {lo}..{hi}"
        );
    }
}

/// A side whose players have more attribute points beats the same club
/// without them (the editor's points are worth spending).
#[test]
fn better_attributes_win_more() {
    let mut jobs = Vec::new();
    for c in [0usize, 2, 3, 5, 7, 8] {
        for seed in 0..16u32 {
            jobs.push((c, seed * 389 + 5, seed % 2 == 0));
        }
    }
    let boosted = |st: &mut Setup, side: usize, by: i32| {
        for p in st.sides[side].players.iter_mut() {
            for a in p.2.iter_mut() {
                *a = (*a as i32 + by).clamp(team::ATTR_MIN as i32, team::ATTR_MAX as i32) as u8;
            }
        }
    };
    let res = par(&jobs, |&(c, seed, home)| {
        let mut st = setup(c, c, seed, HALF);
        let side = if home { 0 } else { 1 };
        boosted(&mut st, side, 15);
        let s = result(&st);
        if home { s } else { [s[1], s[0]] }
    });
    let mut r = Rec::default();
    for s in &res {
        r.add(s[0], s[1]);
    }
    std::println!(
        "+15 attribute points v the same club: W {} D {} L {} (goals {} - {})",
        r.w,
        r.d,
        r.l,
        r.gf,
        r.ga
    );
    assert!(r.w as f32 >= 1.6 * r.l as f32, "{r:?}");
    assert!(r.gf as f32 >= 1.2 * r.ga as f32, "{r:?}");
}

/// Against a human side, the AI's difficulty matters: with the human idle
/// (his active man stands still; his teammates play on), the hard AI
/// outscores the easy one by more.
#[test]
fn the_difficulty_sharpens_the_ai() {
    let mut jobs = Vec::new();
    for difficulty in [0u8, 2] {
        for (h, a) in [(0usize, 1usize), (2, 3), (4, 5), (6, 7), (8, 9)] {
            for seed in 0..4u32 {
                jobs.push((difficulty, h, a, seed * 31 + 3));
            }
        }
    }
    let res = par(&jobs, |&(difficulty, h, a, seed)| {
        let mut st = setup(h, a, seed, HALF);
        st.humans = [true, false];
        st.difficulty = difficulty;
        let mut m = Match::new(&st);
        // An idle human's restarts wait out their timeout first.
        play(&mut m, HALF * 10);
        assert!(m.finished());
        (m.score[1] as i32, m.score[0] as i32)
    });
    let mut gd = [(0i32, 0i32); 2];
    for (&(d, ..), &(f, a)) in jobs.iter().zip(&res) {
        let k = (d / 2) as usize;
        gd[k].0 += f;
        gd[k].1 += a;
    }
    std::println!(
        "AI v an idle human, 20 matches each: easy {} - {}, hard {} - {}",
        gd[0].0,
        gd[0].1,
        gd[1].0,
        gd[1].1
    );
    assert!(gd[1].0 > gd[0].0, "the hard AI scores more: {gd:?}");
    assert!(
        gd[1].0 - gd[1].1 > gd[0].0 - gd[0].1,
        "and wins by more: {gd:?}"
    );
}

/// Three seasons of the league with every club run by the AI: nobody goes
/// broke, nobody runs away with the money, and the books balance.
#[test]
fn the_economy_keeps_every_club_afloat() {
    let mut c = Career::new(0, 77);
    let start = c.clubs[0].budget;
    for season in 0..3 {
        for _ in 0..ROUNDS {
            c.finish_round(None);
        }
        let budgets: Vec<i32> = c.clubs.iter().map(|k| k.budget).collect();
        let (lo, hi) = budgets
            .iter()
            .fold((i32::MAX, i32::MIN), |(a, b), &x| (a.min(x), b.max(x)));
        std::println!(
            "season {}: budgets {lo}..{hi} (champion {})",
            season + 1,
            CLUBS[c.standings()[0] as usize].name
        );
        assert!(lo > 0, "a club went broke: {budgets:?}");
        assert!(hi < start * 6, "runaway money: {budgets:?}");
        assert!(hi < lo * 5, "too unequal: {budgets:?}");
        c.new_season();
    }
    assert_eq!(c.titles.iter().map(|&t| t as u32).sum::<u32>(), 3);
}

/// Transfers are no free money: buying a player and selling him back at
/// once loses coins, and prices follow quality.
#[test]
fn trading_is_never_free_money_and_prices_follow_quality() {
    let mut c = Career::new(0, 3);
    let mut list: Vec<usize> = (0..team::PLAYERS).filter(|&i| c.club_of[i] != 0).collect();
    list.sort_by_key(|&i| c.buy_price(i));
    let i = list[list.len() / 2];
    let before = c.clubs[0].budget;
    c.buy(i).expect("affordable");
    c.sell(i).expect("sellable");
    assert!(
        c.clubs[0].budget < before,
        "a buy-and-sell round trip made money"
    );
    // Higher ratings cost more (rank correlation over the database).
    let mut pairs: Vec<(u8, i32)> = (0..team::PLAYERS)
        .map(|k| (c.players[k].ovr(), c.buy_price(k)))
        .collect();
    pairs.sort_by_key(|p| p.0);
    let mut inversions = 0usize;
    for a in 0..pairs.len() {
        for b in a + 1..pairs.len() {
            if pairs[a].0 < pairs[b].0 && pairs[a].1 > pairs[b].1 {
                inversions += 1;
            }
        }
    }
    let n = pairs.len();
    let total = n * (n - 1) / 2;
    std::println!("price inversions against rating: {inversions} of {total} pairs");
    assert!(inversions * 10 < total, "prices don't follow ratings");
    let _ = ATTRS;
}

/// Seasons simulated from different seeds crown different clubs: the
/// title is open (the career's simulated rounds weigh the squads'
/// strength, but luck and form matter too).
#[test]
fn the_league_title_is_open() {
    let start = Career::new(0, 1);
    let strengths: Vec<i32> = (0..TEAMS as u8).map(|c| start.strength(c)).collect();
    std::println!("strength at the start: {strengths:?}");
    let mut titles = [0u32; TEAMS];
    for seed in 0..20u32 {
        let mut c = Career::new(0, seed * 7919 + 1);
        for _ in 0..ROUNDS {
            c.finish_round(None);
        }
        titles[c.standings()[0] as usize] += 1;
    }
    std::println!("champions over 20 seasons: {titles:?}");
    let distinct = titles.iter().filter(|&&t| t > 0).count();
    assert!(
        distinct >= 5,
        "only {distinct} different champions: {titles:?}"
    );
    assert!(
        titles.iter().all(|&t| t <= 8),
        "one club won too often: {titles:?}"
    );
}

/// Tuning aid: [`neither_side_is_favored`] over four times the matches.
#[test]
#[ignore = "tuning aid"]
fn print_side_bias() {
    let mut jobs = Vec::new();
    for c in 0..TEAMS {
        for seed in 0..40u32 {
            jobs.push((c, seed * 7919 + c as u32 * 13 + 1));
        }
    }
    let res = par(&jobs, |&(c, seed)| result(&setup(c, c, seed, HALF)));
    let mut side = Rec::default();
    for s in &res {
        side.add(s[0], s[1]);
    }
    std::println!(
        "{} mirror matches: side 0 won {}, drew {}, side 1 won {} (goals {} - {})",
        side.played(),
        side.w,
        side.d,
        side.l,
        side.gf,
        side.ga
    );
}
