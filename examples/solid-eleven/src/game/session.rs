//! A match being played: the sim stepped at its fixed tick, the human's
//! pad, the visuals (animations, camera, ball spin), banners for events and
//! the HUD guides.

use super::input::Controls;
use super::sound::{self, Sfx};
use crate::render::anim::AnimState;
use crate::render::clips_gen::ClipName;
use crate::render::hud::{BANNER_SECS, Banner, SideColors};
use crate::render::{self, CamState, Guides, Look, MatchView, SKIN_TONES, Sky, rgb};
use crate::sim::ai;
use crate::sim::ball::{self, DT};
use crate::sim::engine::{EventKind, KickKind, Match, N, Pad, Phase, Setup, side_of};
use crate::sim::fx::{Fx, V2};
use crate::sim::team::{CLUBS, Rgb};
use crate::ui::Ui;
use crate::ui::i18n::{Lang, S};
use trino_core::{Audio, InputState, Quat, Renderer, Vec2, Vec3};

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
/// players. `skins[i]` is player `i`'s skin tone index.
pub fn dress(clubs: [u8; 2], skins: &[u8; N]) -> Dress {
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
        Look {
            shirt: rgb(shirt),
            shorts: if keeper {
                rgb(Rgb(40, 40, 44))
            } else {
                rgb(k.shorts)
            },
            socks: if keeper { rgb(gks[s]) } else { rgb(k.socks) },
            skin: SKIN_TONES[skins[i] as usize % SKIN_TONES.len()],
        }
    });
    Dress {
        looks,
        colors: SideColors {
            shirt: [rgb(kits[0].shirt), rgb(kits[1].shirt)],
            trim: [rgb(kits[0].trim), rgb(kits[1].trim)],
        },
    }
}

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
}

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
            rain: setup.wet,
            time: 0.0,
            guides: Guides::default(),
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
        }
    }

    /// Can the human's controlled player kick now?
    fn human_can_kick(&self) -> bool {
        let Some(s) = self.human else {
            return false;
        };
        let a = self.m.active[s];
        match self.m.phase {
            Phase::Play => self.m.owner == Some(a),
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
                EventKind::Substitution => Some((S::Substitution, Some(e.side))),
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
