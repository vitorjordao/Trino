//! A match being played: the sim stepped at its fixed tick, the human's
//! pad, the visuals (animations, camera, ball spin), banners for events and
//! the HUD guides.

use super::input::Controls;
use super::sound::{self, Sfx};
use crate::render::anim::AnimState;
use crate::render::clips_gen::ClipName;
use crate::render::hud::{BANNER_SECS, Banner, SideColors};
use crate::render::{self, CamState, Guides, Hair, Look, MatchView, SKIN_TONES, Sky, WORLD, rgb};
use crate::sim::ai;
use crate::sim::ball::{self, DT};
use crate::sim::engine::BENCH;
use crate::sim::engine::{Act, EventKind, KickKind, Match, N, Pad, Phase, Setup, side_of};
use crate::sim::fx::{Fx, V2};
use crate::sim::team::{Appearance, SQUAD, generate_player};
use crate::sim::team::{CLUBS, Rgb};
use crate::ui::Ui;
use crate::ui::i18n::{Lang, S};
use trino_core::{Audio, Color, Emitter, InputState, ParticleSystem, Quat, Renderer, Vec2, Vec3};

/// Seconds of sim per tick.
const TICK: f32 = 1.0 / ball::HZ as f32;
/// At most this many ticks per frame (a hitch never fast-forwards).
const MAX_TICKS: u32 = 6;

/// The kit each side wears and each player's looks, chosen at kick-off.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Dress {
    pub looks: [Look; N],
    pub colors: SideColors,
}

/// Pick kits (the away side changes when the shirts clash) and dress the
/// players. `people[i]` is how player `i` looks (skin and hair).
pub fn dress(clubs: [u8; 2], people: &[Appearance; N]) -> Dress {
    let home = &CLUBS[clubs[0] as usize];
    let away = &CLUBS[clubs[1] as usize];
    let away_kit = if away.home.shirt.dist2(home.home.shirt) < 30_000 {
        away.away
    } else {
        away.home
    };
    let kits = [home.home, away_kit];
    let gks = [home.gk, away.gk];
    let looks = core::array::from_fn(|i| {
        let s = side_of(i);
        let k = &kits[s];
        let keeper = i % 11 == 0;
        let shirt: Rgb = if keeper { gks[s] } else { k.shirt };
        Look::of(
            shirt,
            if keeper {
                crate::sim::team::GK_SHORTS
            } else {
                k.shorts
            },
            if keeper { gks[s] } else { k.socks },
            SKIN_TONES[people[i].skin as usize % SKIN_TONES.len()],
        )
        .with_hair(Hair::of(people[i].style, people[i].hair))
    });
    Dress {
        looks,
        colors: SideColors {
            shirt: [rgb(kits[0].shirt), rgb(kits[1].shirt)],
            trim: [rgb(kits[0].trim), rgb(kits[1].trim)],
        },
    }
}

#[derive(Clone)]
pub struct Session {
    pub m: Match,
    pub view: MatchView,
    pub controls: Controls,
    pub colors: SideColors,
    /// Surname index per player (the HUD's tag).
    pub names: [u8; N],
    pub banner: Option<Banner>,
    /// The side a human plays, if any.
    pub human: Option<usize>,
    acc: f32,
    seen: u32,
    prev_officials: [(f32, f32); 3],
    prev_ball: Vec3,
    /// Turf kicked up where a kick or a tackle starts and along a slide
    /// (visual only; the PC tier draws it).
    pub turf: ParticleSystem<TURF_MAX>,
    was_kicking: [bool; N],
    /// Who can come off the bench: database index, surname, look (a
    /// substitute takes his own name and look into the slot).
    pub bench: [(u16, u8, Appearance); 2 * BENCH],
}

/// Consoles never draw the turf, so they keep no room for it (and spend no
/// CPU on it): the session is built on the N64's 64 KB stack.
const CONSOLE: bool = cfg!(any(target_os = "none", target_os = "horizon"));

/// The most turf bits in the air at once.
pub const TURF_MAX: usize = if CONSOLE { 1 } else { 64 };

/// Turf bits: green flecks thrown up and falling back (meters scaled to
/// the drawing scale).
const TURF: Emitter = Emitter {
    sprite: render::S_CONFETTI,
    rate: 0.0,
    // Slowed by the air: every fleck lands as it dies (the slowest, most
    // tilted one is up for 0.51 s).
    life: (0.3, 0.5),
    speed: (1.4 * WORLD, 2.6 * WORLD),
    direction: Vec3::new(0.0, 1.0, 0.0),
    spread: 0.6,
    gravity: Vec3::new(0.0, -4.5 * WORLD, 0.0),
    size: (0.14 * WORLD, 0.08 * WORLD),
    color: (Color::rgba(84, 138, 46, 255), Color::rgba(70, 104, 38, 0)),
    frames: 0,
    frame_size: Vec2::ZERO,
};

impl Session {
    pub fn new(setup: &Setup, dress: &Dress, names: [u8; N], sky: Sky) -> Session {
        let m = Match::new(setup);
        let human = (0..2).find(|&s| setup.humans[s]);
        let mut anims = [AnimState::new(ClipName::Idle); N + 3];
        for (i, a) in anims.iter_mut().enumerate() {
            // Stagger idle cycles so the crowd of players doesn't breathe in step.
            a.time = i as f32 * 0.137;
        }
        let ball = m.ball.pos;
        let view = MatchView {
            anims,
            looks: dress.looks,
            ball_rot: Quat::IDENTITY,
            cam: CamState {
                focus: Vec3::new(ball.x.to_f32(), 0.0, ball.z.to_f32()),
                ..CamState::BROADCAST
            },
            sky,
            stadium: render::Stadium::Arena,
            rain: setup.wet,
            time: 0.0,
            guides: Guides::default(),
            retro: false,
        };
        let officials =
            core::array::from_fn(|k| (m.officials[k].x.to_f32(), m.officials[k].y.to_f32()));
        Session {
            m,
            view,
            controls: Controls::default(),
            colors: dress.colors,
            names,
            banner: None,
            human,
            acc: 0.0,
            seen: 0,
            prev_officials: officials,
            prev_ball: Vec3::ZERO,
            turf: ParticleSystem::new(TURF, 0x7EAF),
            was_kicking: [false; N],
            // The generated squads' (a career sets its own, `set_bench`).
            bench: core::array::from_fn(|k| {
                let (db, ..) = setup.sides[k / BENCH].bench[k % BENCH];
                let p = generate_player(db as usize / SQUAD, db as usize % SQUAD);
                (db, p.surname, p.appearance())
            }),
        }
    }

    /// Can the human's controlled player kick now?
    fn human_can_kick(&self) -> bool {
        let Some(s) = self.human else {
            return false;
        };
        let a = self.m.active[s];
        match self.m.phase {
            Phase::Play => {
                self.m.owner == Some(a)
                    || (self.m.owner.is_none()
                        && (self.m.players[a as usize].pos - self.m.ball.pos.ground()).len()
                            < Fx::milli(1_600))
            }
            Phase::Ready { .. } | Phase::Setup { .. } => {
                self.m.restart.team as usize == s && self.m.taker == a
            }
            _ => false,
        }
    }

    /// One frame: `dt` seconds of play. `touch` carries the 3DS's
    /// bottom-screen taps (player pick, formation) into the pad.
    pub fn update(
        &mut self,
        input: &InputState,
        prev: &InputState,
        audio: &mut dyn Audio,
        dt: f32,
        touch: (u8, u8, bool),
    ) {
        let dt = dt.clamp(0.0, 0.1);
        let can_kick = self.human_can_kick();
        let mut pad = self.controls.pad(input, prev, dt, can_kick);
        pad.pick = touch.0;
        pad.formation = touch.1;
        pad.sub = touch.2;
        self.acc += dt;
        let mut ticks = 0;
        let mut first = true;
        while self.acc >= TICK && ticks < MAX_TICKS {
            self.acc -= TICK;
            ticks += 1;
            // Button edges belong to one tick only; movement to all.
            let p = if first {
                pad
            } else {
                Pad {
                    mv: pad.mv,
                    sprint: pad.sprint,
                    ..Pad::default()
                }
            };
            first = false;
            let mut pads = [Pad::default(); 2];
            if let Some(s) = self.human {
                pads[s] = p;
            }
            self.m.step(pads);
        }
        if ticks == MAX_TICKS {
            self.acc = 0.0;
        }
        self.events(audio);
        self.visuals(dt);
    }

    /// Turn new sim events into banners and sounds.
    fn events(&mut self, audio: &mut dyn Audio) {
        let mut last = None;
        for (_, e) in self.m.events_since(self.seen) {
            let banner = match e.kind {
                EventKind::Goal { own_goal } => {
                    sound::play(audio, Sfx::Cheer);
                    Some((if own_goal { S::OwnGoal } else { S::Goal }, Some(e.side)))
                }
                EventKind::Offside => {
                    sound::play(audio, Sfx::Whistle);
                    Some((S::Offside, Some(e.side)))
                }
                EventKind::Foul => {
                    sound::play(audio, Sfx::Whistle);
                    None
                }
                EventKind::Yellow => Some((S::YellowCard, Some(e.side))),
                EventKind::Red => Some((S::RedCard, Some(e.side))),
                EventKind::Penalty => Some((S::Penalty, Some(e.side))),
                EventKind::Corner => Some((S::Corner, Some(e.side))),
                EventKind::ThrowIn => Some((S::ThrowIn, Some(e.side))),
                EventKind::GoalKick => Some((S::GoalKick, Some(e.side))),
                EventKind::FreeKick => Some((S::FreeKick, Some(e.side))),
                EventKind::HalfTime => {
                    sound::play(audio, Sfx::Whistle);
                    Some((S::HalfTime, None))
                }
                EventKind::FullTime => {
                    sound::play(audio, Sfx::Whistle);
                    Some((S::FullTime, None))
                }
                EventKind::KickOff => {
                    sound::play(audio, Sfx::Whistle);
                    None
                }
                EventKind::Shot => {
                    sound::play(audio, Sfx::Kick);
                    None
                }
                EventKind::Save => {
                    sound::play(audio, Sfx::Ooh);
                    Some((S::Save, Some(e.side)))
                }
                EventKind::Post => {
                    sound::play(audio, Sfx::Post);
                    Some((S::Post, None))
                }
                EventKind::Tackle => {
                    sound::play(audio, Sfx::Tap);
                    None
                }
                EventKind::Substitution => {
                    // The substitute's own name and look.
                    let i = e.player as usize;
                    let db = self.m.players[i].db;
                    if let Some(&(_, name, a)) = self.bench.iter().find(|b| b.0 == db) {
                        self.names[i] = name;
                        let l = &mut self.view.looks[i];
                        l.skin = SKIN_TONES[a.skin as usize % SKIN_TONES.len()];
                        l.hair = Hair::of(a.style, a.hair);
                    }
                    Some((S::Substitution, Some(e.side)))
                }
            };
            // A card outranks the foul's restart banner.
            if let Some(b) = banner {
                let keep = matches!(
                    last,
                    Some((S::YellowCard | S::RedCard | S::Goal | S::OwnGoal, _))
                );
                if !keep {
                    last = Some(b);
                }
            }
        }
        self.seen = self.m.event_seq;
        if let Some((text, side)) = last {
            self.banner = Some(Banner {
                text,
                side,
                left: BANNER_SECS,
            });
        }
    }

    /// Animations, ball spin, camera, guides.
    fn visuals(&mut self, dt: f32) {
        let v = &mut self.view;
        v.time += dt;
        if let Some(b) = &mut self.banner {
            b.left -= dt;
            if b.left <= 0.0 {
                self.banner = None;
            }
        }
        for i in 0..N {
            render::pick_clip(&self.m, i, dt, &mut v.anims[i]);
        }
        // Turf: a burst at the foot where a kick or a tackle starts, a
        // spray along a slide.
        for i in (0..N).filter(|_| !CONSOLE) {
            let p = &self.m.players[i];
            let kick = matches!(p.act, Act::Kick { .. } | Act::Tackle { .. });
            let slide = matches!(p.act, Act::Slide { .. });
            if (kick && !self.was_kicking[i]) || slide {
                let (fx, fz) = (p.face.x.to_f32(), p.face.y.to_f32());
                let at = to3(p.pos) + Vec3::new(fx, 0.0, fz) * 0.35;
                self.turf.origin = Vec3::new(at.x, 0.02, at.z) * WORLD;
                self.turf.burst(if slide { 1 } else { 10 });
            }
            self.was_kicking[i] = kick;
        }
        if !CONSOLE {
            self.turf.update(dt);
        }
        for k in 0..3 {
            render::pick_official_clip(&self.m, k, self.prev_officials[k], dt, &mut v.anims[N + k]);
            self.prev_officials[k] = (
                self.m.officials[k].x.to_f32(),
                self.m.officials[k].y.to_f32(),
            );
        }
        // Ball spin: roll about the axis across its ground velocity.
        let b = self.m.ball;
        let pos = Vec3::new(b.pos.x.to_f32(), b.pos.y.to_f32(), b.pos.z.to_f32());
        let vel = Vec3::new(b.vel.x.to_f32(), b.vel.y.to_f32(), b.vel.z.to_f32());
        let gs = trino_core::math3d::sqrt(vel.x * vel.x + vel.z * vel.z);
        if gs > 0.05 {
            let angle = (gs * dt / 0.11).min(0.9);
            let (s, c) = (
                trino_core::math3d::sin(angle * 0.5),
                trino_core::math3d::cos(angle * 0.5),
            );
            let axis = Vec3::new(vel.z / gs, 0.0, -vel.x / gs);
            let q = Quat::new(axis.x * s, 0.0, axis.z * s, c);
            v.ball_rot = mul_q(q, v.ball_rot).normalized();
        }
        v.cam.follow(pos, vel, dt);
        self.prev_ball = pos;
        // Guides for the human side.
        let mut g = Guides {
            charge: self.controls.charge(),
            ..Guides::default()
        };
        if let Some(s) = self.human
            && matches!(
                self.m.phase,
                Phase::Play | Phase::Ready { .. } | Phase::Setup { .. }
            )
        {
            let a = self.m.active[s];
            g.active = Some(a);
            g.next = ai::switch_target(&self.m, s);
            let ai_ = a as usize;
            let has_ball = self.m.owner == Some(a);
            if has_ball {
                let p = &self.m.players[ai_];
                let dir = p.face;
                if let Some(t) = ai::pass_target(&self.m, ai_, dir) {
                    let q = self.m.players[t as usize].pos;
                    g.pass_lane = Some((to3(self.m.ball.pos.ground()), to3(q)));
                }
                if self.controls.held[1] > 0.0 {
                    // Shot preview at the current charge (on a copy: the
                    // plan draws from the RNG).
                    let mut tmp = self.m.clone();
                    let plan = ai::shot_plan(&mut tmp, ai_, None, Fx::from_f32(g.charge), Fx::ZERO);
                    let mut bb = self.m.ball;
                    bb.vel = plan.vel;
                    bb.spin = plan.spin;
                    let mut n = 0;
                    for _ in 0..10 {
                        for _ in 0..4 {
                            bb.step(self.m.wet);
                        }
                        g.arc[n] =
                            Vec3::new(bb.pos.x.to_f32(), bb.pos.y.to_f32(), bb.pos.z.to_f32());
                        n += 1;
                        if bb.pos.x.abs() > crate::sim::pitch::HALF_LEN {
                            break;
                        }
                    }
                    g.arc_len = n as u8;
                }
            }
        }
        let _ = (KickKind::Pass, DT, V2::ZERO);
        v.guides = g;
    }

    pub fn render(&self, r: &mut dyn Renderer, screen: Vec2, lang: Lang) {
        render::draw_match(r, &self.m, &self.view, screen.x / screen.y.max(1.0));
        if render::hifi(r, self.view.retro) {
            self.turf.draw(r);
        }
        let mut ui = Ui::new(r, screen);
        crate::render::hud::draw(
            &mut ui,
            &self.m,
            &self.colors,
            self.view.guides.active,
            self.view.guides.charge,
            self.banner.as_ref(),
            lang,
            &self.names,
        );
    }
}

fn to3(p: V2) -> Vec3 {
    Vec3::new(p.x.to_f32(), 0.0, p.y.to_f32())
}

/// Quaternion product `a · b` (apply `b` first).
fn mul_q(a: Quat, b: Quat) -> Quat {
    Quat::new(
        a.w * b.x + a.x * b.w + a.y * b.z - a.z * b.y,
        a.w * b.y - a.x * b.z + a.y * b.w + a.z * b.x,
        a.w * b.z + a.x * b.y - a.y * b.x + a.z * b.w,
        a.w * b.w - a.x * b.x - a.y * b.y - a.z * b.z,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest::NullRenderer;

    struct Mute;
    impl Audio for Mute {
        fn play_sound(&mut self, _: trino_core::SoundId) {}
        fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
        fn stop_music(&mut self) {}
        fn set_master_volume(&mut self, _: f32) {}
    }

    /// Counts the turf's billboards (the confetti sprite).
    struct Count {
        caps: trino_core::Caps,
        turf: usize,
    }

    impl Renderer for Count {
        fn caps(&self) -> &trino_core::Caps {
            &self.caps
        }
        fn begin_frame(&mut self, _: trino_core::Color) {}
        fn draw_sprite(&mut self, _: trino_core::SpriteId, _: Vec2, _: &trino_core::SpriteParams) {}
        fn draw_model(
            &mut self,
            _: trino_core::ModelId,
            _: &trino_core::Transform3,
            _: trino_core::Material,
            _: &trino_core::ModelParams,
        ) {
        }
        fn draw_billboard(
            &mut self,
            s: trino_core::SpriteId,
            _: Vec3,
            _: &trino_core::BillboardParams,
        ) {
            if s == render::S_CONFETTI {
                self.turf += 1;
            }
        }
        fn end_frame(&mut self) {}
        fn measure_text(&self, _: trino_core::FontId, t: &str, scale: f32) -> Vec2 {
            NullRenderer { caps: self.caps }.measure_text(trino_core::FontId(0), t, scale)
        }
    }

    /// A substitute comes on with his own name and look (skin, hair), not
    /// the replaced player's.
    #[test]
    fn a_substitute_takes_his_own_name_and_look() {
        let (setup, names, people) = crate::quick_setup(0, 1, 5, 300, [false, false]);
        let d = dress([0, 1], &people);
        let mut s = Session::new(&setup, &d, names, Sky::Day);
        let mut audio = Mute;
        let mut subs = 0;
        while !s.m.finished() {
            let before = s.m.event_seq;
            s.update(
                &InputState::default(),
                &InputState::default(),
                &mut audio,
                1.0 / 30.0,
                (0, 0, false),
            );
            for (_, e) in s.m.events_since(before) {
                if e.kind != EventKind::Substitution {
                    continue;
                }
                let i = e.player as usize;
                let db = s.m.players[i].db as usize;
                let p = generate_player(db / SQUAD, db % SQUAD);
                assert_eq!(s.names[i], p.surname);
                assert_eq!(s.view.looks[i].skin, SKIN_TONES[p.skin as usize]);
                assert_eq!(s.view.looks[i].hair, Hair::of(p.style, p.hair));
                subs += 1;
            }
        }
        assert!(subs > 0, "the AI made no substitution");
    }

    /// Kicks and tackles throw up turf that falls back and dies; the PC
    /// tier draws it, the consoles (and the PC's retro look) never do.
    #[test]
    fn kicks_throw_up_turf_on_the_pc_tier_only() {
        let (setup, names, people) = crate::quick_setup(0, 1, 3, 120, [false, false]);
        let d = dress([0, 1], &people);
        let mut s = Session::new(&setup, &d, names, Sky::Day);
        let mut audio = Mute;
        let (mut most, mut frames_with) = (0, 0);
        for _ in 0..60 * 40 {
            s.update(
                &InputState::default(),
                &InputState::default(),
                &mut audio,
                1.0 / 60.0,
                (0, 0, false),
            );
            most = most.max(s.turf.len());
            if !s.turf.is_empty() {
                frames_with += 1;
                for p in s.turf.particles() {
                    assert!(p.pos.y > -0.05 * WORLD, "turf under the pitch");
                }
            }
        }
        assert!(most > 0 && most <= TURF_MAX);
        assert!(frames_with > 60, "turf in only {frames_with} frames");
        assert!(frames_with < 60 * 40, "turf never settles");
        // Drawn on the PC tier only.
        while s.turf.is_empty() {
            s.update(
                &InputState::default(),
                &InputState::default(),
                &mut audio,
                1.0 / 60.0,
                (0, 0, false),
            );
        }
        let screen = Vec2::new(320.0, 240.0);
        let mut pc = Count {
            caps: trino_core::Caps::PC,
            turf: 0,
        };
        s.render(&mut pc, screen, Lang::En);
        assert_eq!(pc.turf, s.turf.len());
        for caps in [trino_core::Caps::N64, trino_core::Caps::N3DS] {
            let mut console = Count { caps, turf: 0 };
            s.render(&mut console, screen, Lang::En);
            assert_eq!(console.turf, 0);
        }
        s.view.retro = true;
        s.render(&mut pc, screen, Lang::En);
        assert_eq!(pc.turf, s.turf.len(), "the retro look draws none");
    }
}
