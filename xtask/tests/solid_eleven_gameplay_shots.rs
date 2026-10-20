//! Evidence for issue #86's gameplay items, headless (not a gate: the
//! behaviour is tested in `examples/solid-eleven/src/game/scenarios.rs`
//! and `src/sim/tests.rs`):
//!
//! ```sh
//! cargo test -p xtask --test solid_eleven_gameplay_shots -- --ignored --nocapture
//! ```
//!
//! Writes frame sequences to `target/evidence/se-gameplay/` (or
//! `$SE_EVIDENCE_DIR`): the walk back after a goal, the referee coming to
//! show a card, a keeper rushing a 1v1, the kick-off taker walking round
//! the ball, a throw-in with everyone walking into place.

use std::path::PathBuf;

use solid_eleven::SolidElevenGame;
use solid_eleven::render::Sky;
use solid_eleven::sim::ball::Ball;
use solid_eleven::sim::engine::{Act, Pad, Phase, SIDE};
use solid_eleven::sim::fx::{Fx, V2, V3};
use solid_eleven::sim::pitch;
use trino_core::{Game, InputState, Vec2, Vec3};
use trino_platform_pc::{PcRenderer, SimProfile};

#[path = "support/solid_eleven.rs"]
#[allow(dead_code)]
mod support;
use support::{renderer, root, write_png};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn out_dir() -> PathBuf {
    std::env::var_os("SE_EVIDENCE_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root().join("target/evidence/se-gameplay"))
}

struct Shots {
    r: PcRenderer,
    dir: PathBuf,
}

impl Shots {
    fn new(profile: SimProfile, size: Option<(u32, u32)>) -> Shots {
        let mut r = renderer(profile, size);
        r.set_msaa(profile == SimProfile::Native);
        Shots { r, dir: out_dir() }
    }

    fn size(&self) -> Vec2 {
        let (w, h) = self.r.internal_size();
        Vec2::new(w as f32, h as f32)
    }

    /// Render `g` with the camera on `at` (meters), `height` up and `back`
    /// behind, and save it as `name.png`.
    fn shot(&mut self, g: &mut SolidElevenGame, name: &str, at: V2, height: f32, back: f32) {
        self.shot_at(g, name, at, 0.9, height, back);
    }

    /// [`Shots::shot`] looking at `up` meters above the turf.
    fn shot_at(
        &mut self,
        g: &mut SolidElevenGame,
        name: &str,
        at: V2,
        up: f32,
        height: f32,
        back: f32,
    ) {
        let s = g.session.as_mut().unwrap();
        let cam = &mut s.view.cam;
        cam.focus = Vec3::new(at.x.to_f32(), up, at.y.to_f32());
        cam.height = height;
        cam.back = back;
        g.render(&mut self.r);
        let (w, h) = self.r.internal_size();
        let path = self.dir.join(format!("{name}.png"));
        write_png(&path, w, h, &self.r.read_offscreen());
        eprintln!("evidence: {}", path.display());
    }
}

fn game(size: Vec2, seed: u32) -> SolidElevenGame {
    SolidElevenGame::exhibition(size, 0, 1, seed, [false, false], Sky::Day)
}

/// Play `ticks` ticks of the game (one per update) with no input.
fn tick(g: &mut SolidElevenGame, ticks: u32) {
    let mut a = NullAudio;
    for _ in 0..ticks {
        g.update(&InputState::default(), &mut a, 1.0 / 60.0);
    }
}

fn until(g: &mut SolidElevenGame, max: u32, f: impl Fn(&solid_eleven::sim::engine::Match) -> bool) {
    for _ in 0..max {
        if f(&g.session.as_ref().unwrap().m) {
            return;
        }
        tick(g, 1);
    }
}

/// A top-down picture of everyone's path (one dot per tick: side 0 red,
/// side 1 blue, the referee black), the pitch's lines in white: a jump
/// would show as a gap in a trail.
struct Trace {
    w: u32,
    h: u32,
    px: Vec<u8>,
}

impl Trace {
    const SCALE: f32 = 7.0;

    fn new() -> Trace {
        let (hl, hw) = (
            pitch::HALF_LEN.to_f32() + 4.0,
            pitch::HALF_WID.to_f32() + 4.0,
        );
        let (w, h) = (
            (2.0 * hl * Self::SCALE) as u32,
            (2.0 * hw * Self::SCALE) as u32,
        );
        let mut t = Trace {
            w,
            h,
            px: [40u8, 120, 50, 255].repeat((w * h) as usize),
        };
        // Touchlines, goal lines, halfway line, center circle.
        let (l, wd) = (pitch::HALF_LEN.to_f32(), pitch::HALF_WID.to_f32());
        let steps = 2000;
        for k in 0..=steps {
            let f = k as f32 / steps as f32;
            let x = -l + 2.0 * l * f;
            let y = -wd + 2.0 * wd * f;
            for p in [(x, -wd), (x, wd), (-l, y), (l, y), (0.0, y)] {
                t.dot(p.0, p.1, [230, 230, 230]);
            }
            let a = f * std::f32::consts::TAU;
            t.dot(9.15 * a.cos(), 9.15 * a.sin(), [230, 230, 230]);
        }
        t
    }

    fn dot(&mut self, x: f32, y: f32, c: [u8; 3]) {
        let (hl, hw) = (
            pitch::HALF_LEN.to_f32() + 4.0,
            pitch::HALF_WID.to_f32() + 4.0,
        );
        let px = ((x + hl) * Self::SCALE) as i32;
        let py = ((y + hw) * Self::SCALE) as i32;
        if px < 0 || py < 0 || px >= self.w as i32 || py >= self.h as i32 {
            return;
        }
        let i = ((py as u32 * self.w + px as u32) * 4) as usize;
        self.px[i..i + 3].copy_from_slice(&c);
    }

    fn record(&mut self, m: &solid_eleven::sim::engine::Match) {
        for (i, p) in m.players.iter().enumerate() {
            if !p.off {
                let c = if i < SIDE {
                    [230, 40, 40]
                } else {
                    [40, 80, 240]
                };
                self.dot(p.pos.x.to_f32(), p.pos.y.to_f32(), c);
            }
        }
        let r = m.officials[0];
        self.dot(r.x.to_f32(), r.y.to_f32(), [10, 10, 10]);
        let b = m.ball.pos.ground();
        self.dot(b.x.to_f32(), b.y.to_f32(), [255, 255, 255]);
    }

    fn save(&self, name: &str) {
        let path = out_dir().join(format!("{name}.png"));
        write_png(&path, self.w, self.h, &self.px);
        eprintln!("evidence: {}", path.display());
    }
}

fn v2(x: i32, y: i32) -> V2 {
    V2::new(Fx::int(x), Fx::int(y))
}

#[test]
#[ignore = "local tool: evidence frames (needs a GPU)"]
fn goal_then_walk_back() {
    let mut sh = Shots::new(SimProfile::Native, Some((960, 540)));
    let mut g = game(sh.size(), 7);
    until(&mut g, 60 * 30, |m| m.phase == Phase::Play);
    tick(&mut g, 60 * 6);
    let m = &mut g.session.as_mut().unwrap().m;
    let scorer = 9;
    let goal = m.attacks(0);
    m.last_touch = scorer as u8;
    let from = goal.goal_center() - V2::new(goal.sign().mul(Fx::int(3)), Fx::ZERO);
    m.players[scorer].pos = from - V2::new(goal.sign().mul(Fx::int(2)), Fx::int(2));
    let k = m.keeper_of(1).unwrap();
    m.players[k].pos = goal.goal_center() + V2::new(Fx::ZERO, Fx::int(5));
    let mut b = Ball::at_rest(from);
    b.vel = V3::new(goal.sign().mul(Fx::int(20)), Fx::milli(500), Fx::ZERO);
    m.ball = b;
    m.owner = None;
    until(&mut g, 120, |m| matches!(m.phase, Phase::Goal { .. }));
    {
        // The whole celebration and walk back, top-down.
        let mut probe = g.session.as_ref().unwrap().m.clone();
        let mut t = Trace::new();
        while !matches!(probe.phase, Phase::Ready { .. }) {
            t.record(&probe);
            probe.step([Pad::default(); 2]);
        }
        t.record(&probe);
        t.save("goal_trace_topdown");
    }
    let wide = |g: &SolidElevenGame| {
        let _ = g;
        V2::new(goal.sign().mul(Fx::int(20)), Fx::ZERO)
    };
    let at = wide(&g);
    sh.shot(&mut g, "goal_01_scored", at, 30.0, 40.0);
    tick(&mut g, 70);
    sh.shot(&mut g, "goal_02_celebration", at, 30.0, 40.0);
    until(&mut g, 400, |m| matches!(m.phase, Phase::Setup { .. }));
    sh.shot(&mut g, "goal_03_walk_back_starts", at, 30.0, 40.0);
    for k in 4..9 {
        tick(&mut g, 70);
        sh.shot(
            &mut g,
            &format!("goal_{k:02}_walking_back"),
            V2::ZERO,
            34.0,
            44.0,
        );
    }
    until(&mut g, 60 * 20, |m| matches!(m.phase, Phase::Ready { .. }));
    sh.shot(&mut g, "goal_09_kick_off_set", V2::ZERO, 34.0, 44.0);
}

#[test]
#[ignore = "local tool: evidence frames (needs a GPU)"]
fn referee_shows_the_card_up_close() {
    for (profile, tag, size) in [
        (SimProfile::Native, "pc", Some((960u32, 540u32))),
        (SimProfile::N64, "n64", None),
    ] {
        let mut sh = Shots::new(profile, size);
        let mut g = game(sh.size(), 21);
        until(&mut g, 60 * 30, |m| m.phase == Phase::Play);
        let m = &mut g.session.as_mut().unwrap().m;
        // A slide from behind on the carrier: a free kick and a yellow.
        m.humans = [false, true];
        let carrier = 9;
        let tackler = SIDE + 4;
        let at = v2(-5, 6);
        let fwd = m.attacks(0).sign();
        for i in 0..2 * SIDE {
            if i != carrier && i != tackler && (m.players[i].pos - at).len() < Fx::int(16) {
                m.players[i].pos = V2::new(m.players[i].pos.x, -Fx::int(25));
            }
        }
        m.players[carrier].pos = at;
        m.players[carrier].face = V2::new(fwd, Fx::ZERO);
        m.players[carrier].vel = V2::ZERO;
        m.players[carrier].act = Act::Free;
        m.ball = Ball::at_rest(at + V2::new(fwd.mul(Fx::milli(1_500)), Fx::ZERO));
        m.owner = Some(carrier as u8);
        m.players[tackler].pos = at - V2::new(fwd.mul(Fx::milli(1_200)), Fx::ZERO);
        m.players[tackler].face = V2::new(fwd, Fx::ZERO);
        m.players[tackler].act = Act::Free;
        m.active[1] = tackler as u8;
        let pad = Pad {
            slide: true,
            mv: V2::new(fwd, Fx::ZERO),
            ..Pad::default()
        };
        m.step([Pad::default(), pad]);
        until(&mut g, 90, |m| m.phase != Phase::Play);
        if tag == "pc" {
            let mut probe = g.session.as_ref().unwrap().m.clone();
            let mut t = Trace::new();
            while probe.booking.is_some() {
                t.record(&probe);
                probe.step([Pad::default(); 2]);
            }
            t.save("card_trace_topdown");
        }
        let spot = |g: &SolidElevenGame| g.session.as_ref().unwrap().m.players[tackler].pos;
        let p = spot(&g);
        sh.shot(&mut g, &format!("card_01_foul_{tag}"), p, 9.0, 16.0);
        tick(&mut g, 40);
        sh.shot(
            &mut g,
            &format!("card_02_referee_on_his_way_{tag}"),
            p,
            9.0,
            16.0,
        );
        until(&mut g, 60 * 10, |m| m.booking.is_some_and(|b| b.shown > 0));
        tick(&mut g, 20);
        sh.shot(&mut g, &format!("card_03_shown_there_{tag}"), p, 9.0, 16.0);
        let r = g.session.as_ref().unwrap().m.officials[0];
        let mid = V2::new((p.x + r.x).mul(Fx::HALF), (p.y + r.y).mul(Fx::HALF));
        // (Without the banner, which would cover his hand.)
        g.session.as_mut().unwrap().banner = None;
        sh.shot_at(
            &mut g,
            &format!("card_04_close_up_{tag}"),
            mid,
            2.0,
            1.6,
            4.2,
        );
    }
}

/// An attacker through on goal, the defenders chasing behind him, a
/// keeper rated 95 on his line (the scenario test's 1v1).
fn one_on_one(size: Vec2, seed: u32) -> (SolidElevenGame, V2) {
    let mut g = game(size, seed);
    until(&mut g, 60 * 30, |m| m.phase == Phase::Play);
    let m = &mut g.session.as_mut().unwrap().m;
    let end = m.attacks(0);
    let keeper = m.keeper_of(1).unwrap();
    m.players[keeper].attrs[solid_eleven::sim::team::DEFENSE] = 95;
    m.players[keeper].pos = V2::new(end.goal_x() - end.sign().mul(Fx::milli(1_500)), Fx::ZERO);
    let carrier = 9;
    let at = V2::new(end.goal_x() - end.sign().mul(Fx::int(25)), Fx::int(3));
    for j in SIDE..2 * SIDE {
        if j != keeper {
            let y = Fx::int((j % SIDE) as i32 * 4 - 22);
            m.players[j].pos = at - V2::new(end.sign().mul(Fx::int(9)), y);
            m.players[j].vel = V2::ZERO;
        }
    }
    m.players[carrier].pos = at;
    m.players[carrier].vel = V2::new(end.sign().mul(Fx::int(4)), Fx::ZERO);
    m.players[carrier].face = V2::new(end.sign(), Fx::ZERO);
    m.players[carrier].act = Act::Free;
    m.ball = Ball::at_rest(at + V2::new(end.sign().mul(Fx::milli(500)), Fx::ZERO));
    m.owner = Some(carrier as u8);
    m.owned_since = m.tick;
    let view = V2::new(end.goal_x() - end.sign().mul(Fx::int(14)), Fx::ZERO);
    (g, view)
}

#[test]
#[ignore = "local tool: evidence frames (needs a GPU)"]
fn keeper_rushes_a_one_on_one() {
    let mut sh = Shots::new(SimProfile::Native, Some((960, 540)));
    // The first seed where he comes out (the scenario test counts how
    // often), then one where he stays, for comparison.
    let mut done = [false; 2];
    for seed in 100..140u32 {
        let (mut g, _) = one_on_one(sh.size(), seed);
        let mut came = false;
        for _ in 0..100 {
            tick(&mut g, 1);
            came |= g.session.as_ref().unwrap().m.rush[1].1;
        }
        let tag = if came { "comes_out" } else { "stays" };
        if done[came as usize] {
            continue;
        }
        done[came as usize] = true;
        let (mut g, view) = one_on_one(sh.size(), seed);
        for k in 0..8 {
            tick(&mut g, if k == 0 { 1 } else { 12 });
            sh.shot(&mut g, &format!("keeper_{tag}_{k:02}"), view, 12.0, 26.0);
        }
        eprintln!("keeper {tag}: seed {seed}");
        if done == [true, true] {
            return;
        }
    }
    panic!("no seed for both cases: {done:?}");
}

#[test]
#[ignore = "local tool: evidence frames (needs a GPU)"]
fn kicker_walks_round_the_ball() {
    let mut sh = Shots::new(SimProfile::Native, Some((960, 540)));
    let mut g = game(sh.size(), 11);
    until(&mut g, 200, |m| matches!(m.phase, Phase::Ready { .. }));
    let mut k = 0;
    while matches!(g.session.as_ref().unwrap().m.phase, Phase::Ready { .. }) && k < 12 {
        sh.shot(&mut g, &format!("orbit_{k:02}"), V2::ZERO, 2.4, 5.0);
        tick(&mut g, 5);
        k += 1;
    }
    tick(&mut g, 6);
    sh.shot(&mut g, &format!("orbit_{k:02}_kick"), V2::ZERO, 2.4, 5.0);
}

#[test]
#[ignore = "local tool: evidence frames (needs a GPU)"]
fn throw_in_walk_in() {
    let mut sh = Shots::new(SimProfile::Native, Some((960, 540)));
    let mut g = game(sh.size(), 3);
    until(&mut g, 60 * 30, |m| m.phase == Phase::Play);
    let m = &mut g.session.as_mut().unwrap().m;
    m.last_touch = 9;
    let spot = V2::new(Fx::int(10), pitch::HALF_WID - Fx::milli(300));
    let mut b = Ball::at_rest(spot);
    b.vel = V3::new(Fx::ZERO, Fx::ZERO, Fx::int(6));
    m.ball = b;
    m.owner = None;
    until(&mut g, 120, |m| m.phase != Phase::Play);
    {
        let mut probe = g.session.as_ref().unwrap().m.clone();
        let mut t = Trace::new();
        while probe.phase != Phase::Play {
            t.record(&probe);
            probe.step([Pad::default(); 2]);
        }
        t.record(&probe);
        t.save("throw_trace_topdown");
    }
    let view = V2::new(Fx::int(10), Fx::int(22));
    sh.shot(&mut g, "throw_01_out", view, 14.0, 26.0);
    let mut k = 2;
    while g.session.as_ref().unwrap().m.phase != Phase::Play && k < 12 {
        tick(&mut g, 25);
        let name = match g.session.as_ref().unwrap().m.phase {
            Phase::Dead { .. } => "dead",
            Phase::Setup { .. } => "walking_in",
            Phase::Ready { .. } => "thrower_set",
            _ => "thrown",
        };
        sh.shot(&mut g, &format!("throw_{k:02}_{name}"), view, 14.0, 26.0);
        k += 1;
    }
    tick(&mut g, 10);
    sh.shot(&mut g, &format!("throw_{k:02}_thrown"), view, 14.0, 26.0);
}
