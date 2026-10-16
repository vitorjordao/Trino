//! The match: 22 players, a ball and a referee, stepped at 60 Hz in fixed
//! point. One [`Match::step`] = one tick; the game layer runs as many ticks
//! per frame as the frame took (two at 30 fps), so 30 and 60 fps play the
//! same match.
//!
//! Every input is a [`Pad`] (a human side) or the AI ([`super::ai`]); the
//! same pads give the same match on every target, and [`Match::hash`]
//! proves it.

use super::ball::{self, Ball, DT};
use super::fx::{Fx, Hasher, Rng, V2, V3};
use super::pitch::{self, BALL_R, End, HALF_LEN, HALF_WID};
use super::rules::{self, Card, Challenge, Restart, RestartKind};
use super::team::{self, ATTRS, Role, Tactics};

/// Players on the pitch at kick-off.
pub const N: usize = 22;
/// Players per side.
pub const SIDE: usize = 11;

/// Which side player `i` plays for.
#[inline]
pub const fn side_of(i: usize) -> usize {
    i / SIDE
}

/// What a player is doing.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Act {
    Free,
    /// Winding up a kick: the ball leaves when `t` reaches 0.
    Kick {
        t: u8,
        plan: KickPlan,
    },
    /// A standing tackle; decided at its midpoint.
    Tackle {
        t: u8,
    },
    /// A sliding tackle along `dir`; `touched` once the ball was played.
    Slide {
        t: u8,
        dir: V2,
        touched: bool,
    },
    /// On the ground (fouled, or getting up after a slide).
    Down {
        t: u8,
    },
    /// A keeper's dive toward a point, with hands up.
    Dive {
        t: u8,
        vel: V2,
    },
    /// A keeper holding the ball; distributes when `t` reaches 0.
    Hold {
        t: u16,
    },
    /// Scored: celebrating.
    Celebrate,
}

/// Kinds of kick, for stats, offside and the animation.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum KickKind {
    Pass,
    /// Into space ahead of a runner, lofted or along the ground.
    Through,
    Cross,
    Shot,
    Clear,
    /// Throw-in.
    Throw,
    /// A keeper's throw or punt.
    Distribute,
}

/// A kick as planned when the wind-up starts.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct KickPlan {
    pub kind: KickKind,
    pub vel: V3,
    pub spin: Fx,
    /// The intended receiver (passes).
    pub to: Option<u8>,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Player {
    pub pos: V2,
    pub vel: V2,
    /// Unit facing.
    pub face: V2,
    /// Effective attributes for this match (form and morale applied).
    pub attrs: [u8; ATTRS],
    pub role: Role,
    pub number: u8,
    /// Database index (names and looks live outside the sim).
    pub db: u16,
    /// 1 = fresh.
    pub stamina: Fx,
    pub act: Act,
    /// Ticks before he may touch the ball again (after a kick or a lost
    /// duel).
    pub cool: u8,
    pub yellow: u8,
    /// Sent off (off the pitch for the rest of the match).
    pub off: bool,
    pub goals: u8,
    /// Where the AI (or a set piece) wants him.
    pub target: V2,
    /// Sprinting this tick.
    pub sprint: bool,
}

/// One tick's input for a human side, already in pitch directions.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Pad {
    /// Desired movement, |mv| ≤ 1 (pitch x, pitch y).
    pub mv: V2,
    pub sprint: bool,
    /// A kick button was **released** this tick with this charge (0..1).
    pub pass: Option<Fx>,
    pub through: Option<Fx>,
    pub shoot: Option<Fx>,
    /// Pressed this tick.
    pub tackle: bool,
    pub slide: bool,
    pub switch: bool,
}

/// A side as the match sees it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct SideSetup {
    pub club: u8,
    pub tactics: Tactics,
    /// The eleven: attributes, number, database index; slot `i` plays the
    /// formation's slot `i`.
    pub players: [(u16, u8, [u8; ATTRS]); SIDE],
    /// 0..100; 50 = neutral.
    pub morale: u8,
}

/// Everything a match starts from.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Setup {
    pub sides: [SideSetup; 2],
    /// Ticks per half (60 per second of play).
    pub half_ticks: u32,
    pub seed: u32,
    pub wet: bool,
    /// Which sides a human controls.
    pub humans: [bool; 2],
    /// 0 = easy, 1 = normal, 2 = hard: how sharp the AI is.
    pub difficulty: u8,
}

/// Match phases.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Phase {
    /// Players take their places for [`Match::restart`].
    Setup {
        t: u16,
    },
    /// The taker is on the ball; play restarts with his kick.
    Ready {
        t: u16,
    },
    Play,
    /// The whistle went: restart after `t` ticks.
    Dead {
        t: u16,
    },
    /// A goal: celebration, then kick-off.
    Goal {
        t: u16,
        side: u8,
    },
    HalfTime {
        t: u16,
    },
    FullTime,
}

/// Per-side numbers.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Stats {
    pub shots: u16,
    pub on_target: u16,
    pub passes: u16,
    pub completed: u16,
    pub possession: u32,
    pub fouls: u16,
    pub yellows: u8,
    pub reds: u8,
    pub corners: u8,
    pub offsides: u8,
    pub saves: u8,
}

/// Things the UI and the sounds react to.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum EventKind {
    KickOff,
    Goal { own_goal: bool },
    Shot,
    Save,
    Post,
    Offside,
    Foul,
    Yellow,
    Red,
    Corner,
    ThrowIn,
    GoalKick,
    FreeKick,
    Penalty,
    HalfTime,
    FullTime,
    Tackle,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Event {
    pub tick: u32,
    pub kind: EventKind,
    pub side: u8,
    /// The player involved (scorer, offender, ...), 0..22.
    pub player: u8,
}

/// The last [`EVENTS`] events, by sequence number.
pub const EVENTS: usize = 16;

/// Ticks of each pause.
pub const DEAD_TICKS: u16 = 50;
pub const SETUP_TICKS: u16 = 100;
pub const GOAL_TICKS: u16 = 170;
pub const HALF_TIME_TICKS: u16 = 150;
/// A human taker who doesn't kick: the AI takes it after this.
pub const READY_TIMEOUT: u16 = 60 * 8;

/// Movement.
pub const JOG: Fx = Fx::milli(4_800);
pub const ACCEL: Fx = Fx::milli(16_000);
pub const BODY_R: Fx = Fx::milli(400);
/// How close a player must be to take a free ball (feet).
pub const REACH: Fx = Fx::milli(650);
/// Highest ball a foot or chest can control, and a head can play.
pub const FOOT_H: Fx = Fx::milli(1_000);
/// Highest ball a chest can kill (above it, a header).
pub const CHEST_H: Fx = Fx::milli(1_650);
pub const HEAD_H: Fx = Fx::milli(2_300);
/// Keepers' hands, in their own area.
pub const HANDS_H: Fx = Fx::milli(2_600);
pub const HANDS_REACH: Fx = Fx::milli(1_000);

pub const TACKLE_TICKS: u8 = 14;
pub const SLIDE_TICKS: u8 = 36;
pub const DOWN_TICKS: u8 = 50;
pub const DIVE_TICKS: u8 = 34;

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
struct OffsideWatch {
    side: u8,
    /// Bit `i` = player `i` was in an offside position at the kick.
    mask: u32,
}

/// The match state. Fixed size, no heap.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct Match {
    pub players: [Player; N],
    pub ball: Ball,
    /// The player with the ball at his feet (or in a keeper's hands).
    pub owner: Option<u8>,
    pub last_touch: u8,
    pub phase: Phase,
    /// The pending (or current) restart.
    pub restart: Restart,
    /// Who takes [`Match::restart`].
    pub taker: u8,
    /// 1 or 2.
    pub half: u8,
    /// Ticks played in this half.
    pub clock: u32,
    pub half_ticks: u32,
    pub tick: u32,
    /// The end each side defends.
    pub ends: [End; 2],
    pub score: [u8; 2],
    pub stats: [Stats; 2],
    pub tactics: [Tactics; 2],
    pub clubs: [u8; 2],
    pub humans: [bool; 2],
    /// The player each human side controls.
    pub active: [u8; 2],
    pub difficulty: u8,
    pub wet: bool,
    pub rng: Rng,
    /// Referee, then the linesmen on the -y and +y touchlines.
    pub officials: [V2; 3],
    /// A linesman's raised flag: which (1 or 2) and for how long.
    pub flag: (u8, u16),
    events: [Event; EVENTS],
    /// Events logged so far (the next one's sequence number).
    pub event_seq: u32,
    offside: Option<OffsideWatch>,
    /// A pass in flight: (side, passer).
    pass: Option<(u8, u8)>,
    /// A shot in flight, by this side.
    shot: Option<u8>,
    /// When the current owner got the ball.
    pub owned_since: u32,
    /// When the last shot left the boot (keepers react after it).
    pub shot_tick: u32,
    /// The intended receiver of the pass in flight.
    pub receiver: Option<u8>,
    /// The kick that restarts play is exempt from offside.
    restart_kick: Option<RestartKind>,
    /// Which side kicked off the first half.
    first_kick: u8,
    /// Stamina drain per tick of sprinting (scaled by match length).
    drain: Fx,
    /// The AI's memory: when each player last re-planned.
    pub(super) plan_tick: [u8; N],
    /// The free ball's predicted path this tick (shared by the AI).
    pub(super) traj: super::ai::Traj,
    /// Morale (0..100) per side, as set up.
    pub morale: [u8; 2],
    /// Tuning counters (tests only).
    #[cfg(test)]
    pub diag: Diag,
}

/// Where passes end (tests only: the AI tuning reads them).
#[cfg(test)]
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Diag {
    pub ground: u32,
    pub lofted: u32,
    pub to_mate: u32,
    pub to_opp: u32,
    pub dead: u32,
    pub intended: u32,
    pub ground_mate: u32,
    pub ground_opp: u32,
    pub last_lofted: bool,
    /// Carrier decisions: none, shot, pass, through, cross, dribble, clear.
    pub chose: [u32; 7],
}

impl Match {
    pub fn new(setup: &Setup) -> Match {
        let blank = Player {
            pos: V2::ZERO,
            vel: V2::ZERO,
            face: V2::new(Fx::ONE, Fx::ZERO),
            attrs: [50; ATTRS],
            role: Role::Cm,
            number: 0,
            db: 0,
            stamina: Fx::ONE,
            act: Act::Free,
            cool: 0,
            yellow: 0,
            off: false,
            goals: 0,
            target: V2::ZERO,
            sprint: false,
        };
        let mut players = [blank; N];
        for s in 0..2 {
            let side = &setup.sides[s];
            let f = side.tactics.formation();
            let morale = (side.morale as i32 - 50) / 10;
            for k in 0..SIDE {
                let (db, number, attrs) = side.players[k];
                let p = &mut players[s * SIDE + k];
                p.db = db;
                p.number = number;
                p.role = f.role(k);
                for a in 0..ATTRS {
                    p.attrs[a] = (attrs[a] as i32 + morale).clamp(1, 99) as u8;
                }
            }
        }
        let total = (setup.half_ticks * 2).max(600);
        let mut m = Match {
            players,
            ball: Ball::at_rest(V2::ZERO),
            owner: None,
            last_touch: 0,
            phase: Phase::Setup { t: 0 },
            restart: Restart {
                kind: RestartKind::KickOff,
                team: 0,
                spot: V2::ZERO,
            },
            taker: 9,
            half: 1,
            clock: 0,
            half_ticks: setup.half_ticks,
            tick: 0,
            ends: [End::West, End::East],
            score: [0; 2],
            stats: [Stats::default(); 2],
            tactics: [setup.sides[0].tactics, setup.sides[1].tactics],
            clubs: [setup.sides[0].club, setup.sides[1].club],
            humans: setup.humans,
            active: [9, 20],
            difficulty: setup.difficulty,
            wet: setup.wet,
            rng: Rng::new(setup.seed),
            officials: [
                V2::new(Fx::int(-8), Fx::int(-10)),
                V2::new(Fx::ZERO, -HALF_WID - Fx::int(1)),
                V2::new(Fx::ZERO, HALF_WID + Fx::int(1)),
            ],
            flag: (0, 0),
            events: [Event {
                tick: 0,
                kind: EventKind::KickOff,
                side: 0,
                player: 0,
            }; EVENTS],
            event_seq: 0,
            offside: None,
            pass: None,
            shot: None,
            shot_tick: 0,
            owned_since: 0,
            receiver: None,
            restart_kick: None,
            first_kick: 0,
            // A full match of sprinting would empty the tank 1.3 times.
            drain: Fx(((Fx::ONE.0 as i64 * 13 / 10) / total as i64).max(1) as i32),
            plan_tick: [0; N],
            traj: super::ai::Traj::default(),
            morale: [setup.sides[0].morale, setup.sides[1].morale],
            #[cfg(test)]
            diag: Diag::default(),
        };
        m.begin_restart(Restart {
            kind: RestartKind::KickOff,
            team: 0,
            spot: V2::ZERO,
        });
        // Everyone starts in place: no walk-on.
        for i in 0..N {
            m.players[i].pos = m.players[i].target;
        }
        m
    }

    // ------------------------------------------------------------ queries

    /// The end side `s` attacks.
    #[inline]
    pub fn attacks(&self, s: usize) -> End {
        self.ends[s].other()
    }

    /// Attack-x of `p` for side `s` (larger = nearer the goal it attacks).
    #[inline]
    pub fn ax(&self, s: usize, p: V2) -> Fx {
        rules::attack_x(p, self.ends[s])
    }

    /// The side in possession: the owner's, or nobody.
    pub fn possession(&self) -> Option<usize> {
        self.owner.map(|o| side_of(o as usize))
    }

    /// Player on the pitch and able to play.
    #[inline]
    pub fn available(&self, i: usize) -> bool {
        !self.players[i].off
    }

    /// Effective attribute (stamina takes the edge off).
    pub fn attr(&self, i: usize, a: usize) -> Fx {
        let p = &self.players[i];
        let tired = Fx::milli(820) + p.stamina.mul(Fx::milli(180));
        Fx::ratio(p.attrs[a] as i32, 100).mul(tired)
    }

    /// Top speed of player `i` now.
    pub fn top_speed(&self, i: usize, sprint: bool) -> Fx {
        let s = self.attr(i, team::SPEED);
        if sprint {
            Fx::milli(6_300) + s.mul(Fx::milli(2_900))
        } else {
            JOG + s.mul(Fx::milli(600))
        }
    }

    /// The minute shown on the clock (0..90).
    pub fn minute(&self) -> u32 {
        let base = if self.half == 2 { 45 } else { 0 };
        base + (self.clock as u64 * 45 / self.half_ticks.max(1) as u64) as u32
    }

    /// Seconds within the shown minute.
    pub fn second(&self) -> u32 {
        let total = self.clock as u64 * 45 * 60 / self.half_ticks.max(1) as u64;
        (total % 60) as u32
    }

    /// Events logged with sequence numbers `from..` that are still kept.
    pub fn events_since(&self, from: u32) -> impl Iterator<Item = (u32, Event)> + '_ {
        let start = from.max(self.event_seq.saturating_sub(EVENTS as u32));
        (start..self.event_seq).map(|seq| (seq, self.events[seq as usize % EVENTS]))
    }

    pub fn finished(&self) -> bool {
        self.phase == Phase::FullTime
    }

    /// The deterministic state hash (PC, ares and Azahar compare it).
    pub fn hash(&self) -> u32 {
        let mut h = Hasher::new();
        for p in &self.players {
            h.v2(p.pos);
            h.v2(p.vel);
            h.fx(p.stamina);
            h.word(p.yellow as u32 | (p.off as u32) << 8 | (p.cool as u32) << 16);
        }
        h.v3(self.ball.pos);
        h.v3(self.ball.vel);
        h.fx(self.ball.spin);
        h.word(self.owner.map_or(255, |o| o as u32));
        h.word(self.score[0] as u32 | (self.score[1] as u32) << 8);
        h.word(self.tick);
        h.word(self.rng.0);
        h.word(self.event_seq);
        h.0
    }

    pub(super) fn log(&mut self, kind: EventKind, side: usize, player: u8) {
        let e = Event {
            tick: self.tick,
            kind,
            side: side as u8,
            player,
        };
        self.events[self.event_seq as usize % EVENTS] = e;
        self.event_seq += 1;
    }

    // --------------------------------------------------------- the tick

    /// Advance one tick. `pads[s]` drives side `s`'s active player when a
    /// human plays it.
    pub fn step(&mut self, pads: [Pad; 2]) {
        self.tick += 1;
        match self.phase {
            Phase::FullTime => {
                self.idle_players();
            }
            Phase::HalfTime { t } => {
                self.idle_players();
                if t == 0 {
                    self.second_half();
                } else {
                    self.phase = Phase::HalfTime { t: t - 1 };
                }
            }
            Phase::Goal { t, side } => {
                self.celebrate(side as usize);
                self.move_players(&[None; N]);
                if t == 0 {
                    self.begin_restart(Restart {
                        kind: RestartKind::KickOff,
                        team: 1 - side,
                        spot: V2::ZERO,
                    });
                } else {
                    self.phase = Phase::Goal { t: t - 1, side };
                }
                self.ball.step(self.wet);
            }
            Phase::Dead { t } => {
                self.drift_players();
                self.ball.step(self.wet);
                if t == 0 {
                    if self.clock >= self.half_ticks {
                        self.end_half();
                    } else {
                        let r = self.restart;
                        self.begin_restart(r);
                    }
                } else {
                    self.phase = Phase::Dead { t: t - 1 };
                }
            }
            Phase::Setup { t } => {
                self.setup_players();
                let ready = (0..N).all(|i| {
                    self.players[i].off
                        || (self.players[i].pos - self.players[i].target).shorter_than(Fx::int(1))
                });
                if t >= SETUP_TICKS || (ready && t > 20) {
                    self.phase = Phase::Ready { t: 0 };
                    self.snap_taker();
                } else {
                    self.phase = Phase::Setup { t: t + 1 };
                }
            }
            Phase::Ready { t } => {
                self.hold_restart_positions();
                let s = self.restart.team as usize;
                let human = self.humans[s];
                let mut plan = None;
                if human && t < READY_TIMEOUT {
                    // The taker aims with the stick, kicks with a button.
                    let pad = pads[s];
                    if pad.mv != V2::ZERO {
                        let p = &mut self.players[self.taker as usize];
                        p.face = p.face.turn_toward(pad.mv.norm(), Fx::milli(150));
                    }
                    plan = self.human_kick(self.taker as usize, &pad);
                } else if t >= 45 {
                    plan = Some(self.ai_restart_kick());
                }
                // Keep the ball on the spot under the taker's control.
                self.owner = Some(self.taker);
                self.ball = Ball::at_rest(self.restart.spot);
                if self.restart.kind == RestartKind::ThrowIn {
                    self.ball.pos.y = Fx::milli(2_100);
                }
                match plan {
                    Some(plan) => {
                        self.restart_kick = Some(self.restart.kind);
                        let k = self.taker as usize;
                        self.players[k].act = Act::Kick {
                            t: if self.restart.kind == RestartKind::Penalty {
                                14
                            } else {
                                8
                            },
                            plan,
                        };
                        self.phase = Phase::Play;
                    }
                    None => self.phase = Phase::Ready { t: t + 1 },
                }
            }
            Phase::Play => self.play_tick(pads),
        }
        self.move_officials();
        if self.flag.1 > 0 {
            self.flag.1 -= 1;
        }
    }

    fn play_tick(&mut self, pads: [Pad; 2]) {
        self.clock += 1;
        // Commands: AI for everyone, then the humans' active players.
        let mut cmds = [None; N];
        super::ai::think(self, &mut cmds);
        for s in 0..2 {
            if self.humans[s] {
                self.human_tick(s, &pads[s], &mut cmds);
            }
        }
        self.move_players(&cmds);
        self.resolve_actions();
        if self.phase != Phase::Play {
            return;
        }
        self.ball_tick();
        if self.phase != Phase::Play {
            return;
        }
        self.referee();
        if let Some(s) = self.possession() {
            self.stats[s].possession += 1;
        }
        // Time is up: blow at once (no stoppage time).
        if self.phase == Phase::Play && self.clock >= self.half_ticks {
            self.whistle_end_of_half();
        }
    }

    // ------------------------------------------------------- movement

    /// Move everyone toward their command (or their target).
    pub(super) fn move_players(&mut self, cmds: &[Option<Cmd>; N]) {
        for i in 0..N {
            if self.players[i].off {
                continue;
            }
            let cmd = cmds[i].unwrap_or_else(|| {
                // No command: walk to the target.
                let d = self.players[i].target - self.players[i].pos;
                let (dir, len) = d.norm_len();
                Cmd::go(
                    dir.scale(len.min(Fx::int(2)).div(Fx::int(2))),
                    len > Fx::int(8),
                )
            });
            self.move_one(i, &cmd);
            if let Some(a) = cmd.act {
                self.start_action(i, a);
            }
        }
        self.separate();
    }

    fn move_one(&mut self, i: usize, cmd: &Cmd) {
        let sprint = cmd.sprint && self.players[i].stamina > Fx::milli(120);
        let top = self.top_speed(i, sprint);
        let p = &mut self.players[i];
        p.sprint = sprint;
        // Actions that own the body.
        match p.act {
            Act::Slide { t, dir, .. } => {
                let k = Fx::ratio(t as i32, SLIDE_TICKS as i32);
                p.vel = dir.scale(Fx::milli(2_000) + Fx::milli(5_400).mul(k));
                p.pos += p.vel.scale(DT);
                p.pos = pitch::clamp_to_surround(p.pos);
                return;
            }
            Act::Down { .. } | Act::Celebrate | Act::Hold { .. } => {
                p.vel = p.vel.scale(Fx::milli(850));
                p.pos += p.vel.scale(DT);
                if matches!(p.act, Act::Celebrate) {
                    // Celebrations run on their own targets.
                } else {
                    return;
                }
            }
            Act::Dive { vel, .. } => {
                p.vel = vel;
                p.pos += p.vel.scale(DT);
                return;
            }
            _ => {}
        }
        let kicking = matches!(p.act, Act::Kick { .. } | Act::Tackle { .. });
        let mut want = cmd.mv.clamp_len(Fx::ONE).scale(top);
        if kicking {
            want = want.scale(Fx::milli(300));
        }
        let dv = want - p.vel;
        let step = ACCEL.mul(DT);
        p.vel += dv.clamp_len(step);
        p.pos += p.vel.scale(DT);
        p.pos = pitch::clamp_to_surround(p.pos);
        // Face where he runs, or the command's facing.
        let face_to = cmd.face.unwrap_or_else(|| {
            if p.vel.shorter_than(Fx::milli(400)) {
                p.face
            } else {
                p.vel.norm()
            }
        });
        if !kicking {
            p.face = p.face.turn_toward(face_to, Fx::milli(200));
        }
        // Stamina: sprinting drains, jogging a little, walking recovers.
        let speed_sq = p.vel.len_sq_raw();
        let jog_sq = JOG.0 as i64 * JOG.0 as i64;
        let drain = self.drain;
        let stam = Fx::ratio(p.attrs[team::STAMINA] as i32, 100);
        if sprint && speed_sq > jog_sq {
            p.stamina -= drain.mul(Fx::milli(1_500) - stam);
        } else if speed_sq > jog_sq / 4 {
            p.stamina -= drain.mul(Fx::milli(250));
        } else {
            p.stamina += drain.mul(Fx::milli(600));
        }
        p.stamina = p.stamina.clamp(Fx::milli(150), Fx::ONE);
    }

    /// Bodies don't overlap: push pairs apart.
    fn separate(&mut self) {
        let min = BODY_R * 2;
        for a in 0..N {
            if self.players[a].off {
                continue;
            }
            for b in a + 1..N {
                if self.players[b].off {
                    continue;
                }
                let d = self.players[b].pos - self.players[a].pos;
                if (d.x.abs() >= min) || (d.y.abs() >= min) || !d.shorter_than(min) {
                    continue;
                }
                let (n, len) = d.norm_len();
                let n = if n == V2::ZERO {
                    V2::new(Fx::ONE, Fx::ZERO)
                } else {
                    n
                };
                let push = (min - len).mul(Fx::HALF);
                self.players[a].pos -= n.scale(push);
                self.players[b].pos += n.scale(push);
            }
        }
    }

    /// Between phases: walk to targets.
    fn drift_players(&mut self) {
        for i in 0..N {
            if self.players[i].off {
                continue;
            }
            let d = self.players[i].target - self.players[i].pos;
            let (dir, len) = d.norm_len();
            let cmd = Cmd::go(dir.scale(len.min(Fx::int(3)).div(Fx::int(3))), false);
            self.move_one(i, &cmd);
        }
        self.separate();
    }

    fn idle_players(&mut self) {
        for p in self.players.iter_mut() {
            p.vel = p.vel.scale(Fx::milli(900));
            p.pos += p.vel.scale(DT);
            if !matches!(p.act, Act::Celebrate) {
                p.act = Act::Free;
            }
        }
    }

    // ------------------------------------------------------- actions

    fn start_action(&mut self, i: usize, a: CmdAct) {
        let p = self.players[i];
        if !matches!(p.act, Act::Free) {
            return;
        }
        match a {
            CmdAct::Kick(plan) => {
                if self.owner == Some(i as u8) {
                    self.players[i].act = Act::Kick { t: 7, plan };
                }
            }
            CmdAct::Tackle => {
                self.players[i].act = Act::Tackle { t: TACKLE_TICKS };
            }
            CmdAct::Slide(dir) => {
                let dir = if dir == V2::ZERO { p.face } else { dir.norm() };
                self.players[i].act = Act::Slide {
                    t: SLIDE_TICKS,
                    dir,
                    touched: false,
                };
                self.players[i].face = dir;
            }
            CmdAct::Dive(to) => {
                let d = to - p.pos;
                let ticks = Fx::int(DIVE_TICKS as i32 / 2);
                let vel = d
                    .scale(Fx::ONE.div(ticks.mul(DT)))
                    .clamp_len(Fx::milli(4_600));
                self.players[i].act = Act::Dive { t: DIVE_TICKS, vel };
            }
        }
    }

    /// Advance timed actions; kicks, tackles and slides act on the ball.
    fn resolve_actions(&mut self) {
        for i in 0..N {
            if self.players[i].off {
                continue;
            }
            if self.players[i].cool > 0 {
                self.players[i].cool -= 1;
            }
            match self.players[i].act {
                Act::Free | Act::Celebrate => {}
                Act::Kick { t, plan } => {
                    if t == 0 {
                        self.players[i].act = Act::Free;
                        if self.owner == Some(i as u8) {
                            self.release(i, plan);
                        }
                    } else {
                        self.players[i].act = Act::Kick { t: t - 1, plan };
                    }
                }
                Act::Tackle { t } => {
                    if t == TACKLE_TICKS / 2 {
                        self.resolve_tackle(i);
                    }
                    self.players[i].act = if t == 0 {
                        Act::Free
                    } else {
                        Act::Tackle { t: t - 1 }
                    };
                }
                Act::Slide { t, dir, touched } => {
                    let touched = touched || self.slide_contact(i, dir, t);
                    if self.phase != Phase::Play {
                        return;
                    }
                    self.players[i].act = if t == 0 {
                        Act::Down { t: 24 }
                    } else {
                        Act::Slide {
                            t: t - 1,
                            dir,
                            touched,
                        }
                    };
                }
                Act::Down { t } => {
                    self.players[i].act = if t == 0 {
                        Act::Free
                    } else {
                        Act::Down { t: t - 1 }
                    };
                }
                Act::Dive { t, .. } => {
                    if t == 0 {
                        self.players[i].act = Act::Down { t: 20 };
                        self.players[i].vel = V2::ZERO;
                    } else if let Act::Dive { vel, .. } = self.players[i].act {
                        let vel = if t < DIVE_TICKS / 2 {
                            vel.scale(Fx::milli(800))
                        } else {
                            vel
                        };
                        self.players[i].act = Act::Dive { t: t - 1, vel };
                    }
                }
                Act::Hold { t } => {
                    if self.owner != Some(i as u8) {
                        self.players[i].act = Act::Free;
                    } else if t == 0 {
                        let plan = super::ai::keeper_distribution(self, i);
                        self.players[i].act = Act::Kick { t: 8, plan };
                    } else {
                        self.players[i].act = Act::Hold { t: t - 1 };
                    }
                }
            }
        }
    }

    /// The ball leaves player `i` with `plan`.
    pub(super) fn release(&mut self, i: usize, plan: KickPlan) {
        let s = side_of(i);
        let p = self.players[i];
        let mut vel = plan.vel;
        // A throw-in leaves from over the head; kicks from the feet, a
        // little ahead of the kicker.
        let from = if plan.kind == KickKind::Throw {
            V3::from_ground(p.pos + p.face.scale(Fx::milli(300)), Fx::milli(2_000))
        } else if plan.kind == KickKind::Distribute && matches!(vel.y, y if y > Fx::int(4)) {
            V3::from_ground(p.pos + p.face.scale(Fx::milli(400)), Fx::milli(900))
        } else {
            V3::from_ground(self.ball.pos.ground(), BALL_R)
        };
        if vel.len_sq_raw() == 0 {
            vel = V3::from_ground(p.face, Fx::ZERO);
        }
        self.ball.pos = from;
        self.ball.vel = vel;
        self.ball.spin = plan.spin;
        self.owner = None;
        self.receiver = plan.to;
        self.players[i].cool = 12;
        self.last_touch = i as u8;
        // Offside: who was beyond the line at the kick? (Not from
        // throw-ins, goal kicks and corners.)
        let exempt = self
            .restart_kick
            .take()
            .is_some_and(|k| k.exempt_from_offside());
        self.offside = if exempt {
            None
        } else {
            Some(OffsideWatch {
                side: s as u8,
                mask: self.offside_mask(s, i),
            })
        };
        match plan.kind {
            KickKind::Pass
            | KickKind::Through
            | KickKind::Cross
            | KickKind::Throw
            | KickKind::Distribute => {
                self.stats[s].passes += 1;
                self.pass = Some((s as u8, i as u8));
                self.shot = None;
                #[cfg(test)]
                {
                    self.diag.last_lofted = vel.y > Fx::int(2);
                    if vel.y > Fx::int(2) {
                        self.diag.lofted += 1;
                    } else {
                        self.diag.ground += 1;
                    }
                }
            }
            KickKind::Shot => {
                self.stats[s].shots += 1;
                self.shot = Some(s as u8);
                self.shot_tick = self.tick;
                self.pass = None;
                self.log(EventKind::Shot, s, i as u8);
            }
            KickKind::Clear => {
                self.pass = None;
                self.shot = None;
            }
        }
    }

    /// Bit `j` set: teammate `j` of the kicker `k` is in an offside
    /// position now.
    fn offside_mask(&self, s: usize, k: usize) -> u32 {
        let o = 1 - s;
        let mut opps = [Fx::ZERO; SIDE];
        let mut n = 0;
        for j in o * SIDE..o * SIDE + SIDE {
            if !self.players[j].off {
                opps[n] = self.ax(s, self.players[j].pos);
                n += 1;
            }
        }
        let line = rules::second_last(&opps[..n]);
        let ball = self.ax(s, self.ball.pos.ground());
        let mut mask = 0;
        for j in s * SIDE..s * SIDE + SIDE {
            if j == k || self.players[j].off {
                continue;
            }
            if rules::offside_position(self.ax(s, self.players[j].pos), ball, line) {
                mask |= 1 << j;
            }
        }
        mask
    }

    /// Player `i` touches the ball (takes it, heads it, saves it): the
    /// offside watch, pass completion, and the human's player switch.
    /// Returns `false` when the touch was an offside offence (play stops).
    pub(super) fn touch(&mut self, i: usize) -> bool {
        let s = side_of(i);
        if let Some(w) = self.offside {
            if w.side as usize == s {
                if w.mask & (1 << i) != 0 && self.last_touch as usize != i {
                    self.call_offside(i);
                    return false;
                }
            } else {
                // The opponents played it: a new phase of play.
                self.offside = None;
            }
        }
        #[cfg(test)]
        if let Some((ps, from)) = self.pass
            && from as usize != i
        {
            if !self.diag.last_lofted {
                if ps as usize == s {
                    self.diag.ground_mate += 1;
                } else {
                    self.diag.ground_opp += 1;
                }
            }
            if ps as usize == s {
                self.diag.to_mate += 1;
                if self.receiver == Some(i as u8) {
                    self.diag.intended += 1;
                }
            } else {
                self.diag.to_opp += 1;
            }
        }
        if let Some((ps, from)) = self.pass.take()
            && ps as usize == s
            && from as usize != i
        {
            self.stats[s].completed += 1;
        }
        if let Some(ss) = self.shot.take()
            && ss as usize != s
            && self.players[i].role == Role::Gk
        {
            self.stats[ss as usize].on_target += 1;
            self.stats[s].saves += 1;
            self.log(EventKind::Save, s, i as u8);
        }
        self.last_touch = i as u8;
        self.receiver = None;
        if self.humans[s] && self.players[i].role != Role::Gk {
            self.active[s] = i as u8;
        }
        true
    }

    fn call_offside(&mut self, i: usize) {
        let s = side_of(i);
        let spot = pitch::clamp_to_field(self.players[i].pos);
        self.stats[s].offsides += 1;
        self.log(EventKind::Offside, s, i as u8);
        // The linesman on that half's touchline raises his flag.
        let lm = if spot.x.mul(self.ends[s].sign()) < Fx::ZERO {
            1
        } else {
            2
        };
        self.flag = (lm, 120);
        self.whistle(Restart {
            kind: RestartKind::FreeKick { direct: false },
            team: (1 - s) as u8,
            spot,
        });
    }

    /// Stop play; restart with `r` after the pause.
    pub(super) fn whistle(&mut self, r: Restart) {
        #[cfg(test)]
        if self.pass.is_some() {
            self.diag.dead += 1;
        }
        self.restart = r;
        self.owner = None;
        self.offside = None;
        self.pass = None;
        self.shot = None;
        self.receiver = None;
        self.phase = Phase::Dead { t: DEAD_TICKS };
        for p in self.players.iter_mut() {
            if matches!(
                p.act,
                Act::Kick { .. } | Act::Tackle { .. } | Act::Hold { .. }
            ) {
                p.act = Act::Free;
            }
        }
        let kind = match r.kind {
            RestartKind::Corner => Some(EventKind::Corner),
            RestartKind::ThrowIn => Some(EventKind::ThrowIn),
            RestartKind::GoalKick => Some(EventKind::GoalKick),
            RestartKind::Penalty => Some(EventKind::Penalty),
            RestartKind::FreeKick { .. } => Some(EventKind::FreeKick),
            RestartKind::KickOff => None,
        };
        if let Some(k) = kind {
            if r.kind == RestartKind::Corner {
                self.stats[r.team as usize].corners += 1;
            }
            self.log(k, r.team as usize, self.last_touch);
        }
    }

    fn resolve_tackle(&mut self, i: usize) {
        let Some(o) = self.owner else {
            return;
        };
        let o = o as usize;
        if side_of(o) == side_of(i) || matches!(self.players[o].act, Act::Hold { .. }) {
            return;
        }
        let me = self.players[i];
        let foot = me.pos + me.face.scale(Fx::milli(550));
        if !(foot - self.ball.pos.ground()).shorter_than(Fx::milli(900)) {
            return;
        }
        let victim = self.players[o];
        // Duel: defense against dribbling, worse from behind.
        let behind = me.face.dot(victim.face) > Fx::milli(500);
        let mut p = Fx::milli(420)
            + (self.attr(i, team::DEFENSE) - self.attr(o, team::DRIBBLE)).mul(Fx::milli(700));
        if behind {
            p -= Fx::milli(200);
        }
        p = p.clamp(Fx::milli(120), Fx::milli(880));
        if self.rng.chance(p) {
            // Won: the ball pops loose toward the tackler's facing.
            self.owner = None;
            self.ball.vel = V3::from_ground(
                me.face.scale(Fx::int(4)) + victim.vel.scale(Fx::milli(300)),
                Fx::ZERO,
            );
            self.players[o].cool = 18;
            self.log(EventKind::Tackle, side_of(i), i as u8);
            self.touch(i);
            return;
        }
        // Lost: a foul if it went through the man.
        let aggression = Fx::ratio(self.tactics[side_of(i)].aggression as i32, 100);
        let foul_p = if behind {
            Fx::milli(350) + aggression.mul(Fx::milli(300))
        } else {
            aggression.mul(Fx::milli(120))
        };
        if self.rng.chance(foul_p) {
            self.foul(i, o, false);
        } else {
            self.players[i].cool = 20;
        }
    }

    /// A slide's feet touch the ball or a man. Returns `true` once the
    /// ball was played.
    fn slide_contact(&mut self, i: usize, dir: V2, t: u8) -> bool {
        if !(4..=SLIDE_TICKS - 3).contains(&t) {
            return false;
        }
        let me = self.players[i];
        let feet = me.pos + dir.scale(Fx::milli(700));
        let s = side_of(i);
        let bp = self.ball.pos.ground();
        let ball_low = self.ball.pos.y < Fx::milli(600);
        if ball_low && (feet - bp).shorter_than(Fx::milli(650)) {
            let owner = self.owner;
            if let Some(o) = owner {
                if side_of(o as usize) == s
                    || matches!(self.players[o as usize].act, Act::Hold { .. })
                {
                    return false;
                }
                self.players[o as usize].cool = 24;
                self.players[o as usize].act = Act::Down { t: 30 };
            }
            if let Act::Slide { touched: true, .. } = me.act {
                return true;
            }
            // Clean: the ball goes on the slide's line.
            self.owner = None;
            self.ball.pos = V3::from_ground(bp, BALL_R);
            self.ball.vel = V3::from_ground(
                dir.scale(Fx::int(7)) + dir.perp().scale(self.rng.signed()),
                Fx::ZERO,
            );
            self.log(EventKind::Tackle, s, i as u8);
            self.touch(i);
            return true;
        }
        if let Act::Slide { touched: true, .. } = me.act {
            return true;
        }
        // Into an opponent first: foul.
        for j in (1 - s) * SIDE..(1 - s) * SIDE + SIDE {
            let v = &self.players[j];
            if v.off || matches!(v.act, Act::Down { .. }) {
                continue;
            }
            if (feet - v.pos).shorter_than(Fx::milli(600)) {
                self.foul(i, j, true);
                return false;
            }
        }
        false
    }

    /// Player `i` fouls player `v`.
    pub(super) fn foul(&mut self, i: usize, v: usize, slide: bool) {
        let s = side_of(i);
        let vs = side_of(v);
        let me = self.players[i];
        let victim = self.players[v];
        let challenge = Challenge {
            slide,
            from_behind: me.face.dot(victim.face) > Fx::milli(600),
            speed: me.vel.len(),
            dogso: self.owner == Some(v as u8) && self.through_on_goal(v),
        };
        let at = victim.pos;
        let in_box = pitch::in_penalty_area(at, self.ends[s]);
        let card = rules::sanction(challenge, in_box);
        self.stats[s].fouls += 1;
        self.log(EventKind::Foul, s, i as u8);
        let (yellows, red) = rules::accumulate(me.yellow, card);
        self.players[i].yellow = yellows;
        if card == Card::Yellow {
            self.stats[s].yellows += 1;
            self.log(EventKind::Yellow, s, i as u8);
        }
        if red {
            self.stats[s].reds += 1;
            self.log(EventKind::Red, s, i as u8);
            self.send_off(i);
        }
        self.players[v].act = Act::Down { t: DOWN_TICKS };
        self.players[v].vel = me.vel.scale(Fx::milli(400));
        let r = rules::foul_restart(at, vs as u8, self.ends[s]);
        self.whistle(r);
    }

    /// The victim had the ball with only the keeper (or nobody) between
    /// him and goal, within 30 m of it.
    fn through_on_goal(&self, v: usize) -> bool {
        let s = side_of(v);
        let goal = self.attacks(s).goal_center();
        let p = self.players[v].pos;
        if !(goal - p).shorter_than(Fx::int(24)) || p.y.abs() > Fx::int(16) {
            return false;
        }
        // Heading for goal, not away from it.
        if self.players[v].vel.dot((goal - p).norm()) < Fx::int(2) {
            return false;
        }
        let me = self.ax(s, p);
        let o = 1 - s;
        !(o * SIDE..o * SIDE + SIDE).any(|j| {
            let d = &self.players[j];
            !d.off
                && d.role != Role::Gk
                && !matches!(d.act, Act::Slide { .. })
                && self.ax(s, d.pos) > me
        })
    }

    fn send_off(&mut self, i: usize) {
        let p = &mut self.players[i];
        p.off = true;
        p.act = Act::Free;
        p.vel = V2::ZERO;
        // Walks off at the halfway line.
        p.pos = V2::new(Fx::ZERO, HALF_WID + Fx::int(6));
        for s in 0..2 {
            if self.active[s] as usize == i {
                self.active[s] = super::ai::nearest_to_ball(self, s, None).unwrap_or(i as u8);
            }
        }
    }

    // ---------------------------------------------------------- the ball

    fn ball_tick(&mut self) {
        if let Some(o) = self.owner {
            let o = o as usize;
            let p = self.players[o];
            if let Act::Hold { .. } = p.act {
                // In the keeper's hands.
                self.ball.pos =
                    V3::from_ground(p.pos + p.face.scale(Fx::milli(350)), Fx::milli(1_000));
                self.ball.vel = V3::from_ground(p.vel, Fx::ZERO);
                return;
            }
            // Dribbling: the ball runs a touch ahead of the feet, further
            // at speed and for weaker dribblers.
            let speed = p.vel.len();
            let loose = Fx::ONE - self.attr(o, team::DRIBBLE).mul(Fx::milli(600));
            let ahead = Fx::milli(420) + speed.mul(Fx::milli(70)).mul(Fx::milli(600) + loose);
            let want = p.pos + p.face.scale(ahead);
            let cur = self.ball.pos.ground();
            let next = cur + (want - cur).scale(Fx::milli(450));
            self.ball.vel = V3::from_ground((next - cur).scale(Fx::int(ball::HZ)), Fx::ZERO);
            self.ball.pos = V3::from_ground(next, BALL_R);
            self.ball.spin = Fx::ZERO;
            // A ball run out of play while dribbling.
            self.check_out_or_goal();
            return;
        }
        let ev = self.ball.step(self.wet);
        if ev.hit_frame {
            let s = side_of(self.last_touch as usize);
            self.log(EventKind::Post, s, self.last_touch);
        }
        if self.check_out_or_goal() {
            return;
        }
        self.claim_free_ball();
    }

    /// Goal or out of play. Returns `true` when play stopped.
    fn check_out_or_goal(&mut self) -> bool {
        if let Some(end) = ball::goal_scored(&self.ball) {
            let scorer_side = if self.ends[0] == end { 1 } else { 0 };
            self.goal(scorer_side);
            return true;
        }
        let last_side = side_of(self.last_touch as usize) as u8;
        if let Some(r) = rules::out_of_play(
            self.ball.pos.ground(),
            self.ball.pos.y,
            last_side,
            self.ends,
        ) {
            // The shot that went wide or over.
            self.shot = None;
            if self.owner.is_some() {
                // Dribbled out: the ball is loose over the line.
                self.owner = None;
            }
            let lm = if r.spot.y < Fx::ZERO { 1 } else { 2 };
            self.flag = (lm, 70);
            self.whistle(r);
            return true;
        }
        false
    }

    fn goal(&mut self, side: usize) {
        let toucher = self.last_touch as usize;
        let own = side_of(toucher) != side;
        self.score[side] += 1;
        if let Some(ss) = self.shot.take()
            && ss as usize == side
        {
            self.stats[side].on_target += 1;
        }
        if !own {
            self.players[toucher].goals += 1;
        }
        self.log(EventKind::Goal { own_goal: own }, side, toucher as u8);
        self.owner = None;
        self.offside = None;
        self.pass = None;
        self.phase = Phase::Goal {
            t: GOAL_TICKS,
            side: side as u8,
        };
        for p in self.players.iter_mut() {
            if !matches!(p.act, Act::Down { .. }) {
                p.act = Act::Free;
            }
        }
        if !own {
            self.players[toucher].act = Act::Celebrate;
        }
    }

    /// A loose ball: the nearest able player controls, heads or (a keeper
    /// in his area) catches it.
    fn claim_free_ball(&mut self) {
        let bp = self.ball.pos.ground();
        let h = self.ball.pos.y;
        if h > HANDS_H + Fx::int(1) {
            return;
        }
        let mut best: Option<(usize, i64)> = None;
        for i in 0..N {
            let p = &self.players[i];
            if p.off || p.cool > 0 || matches!(p.act, Act::Down { .. } | Act::Slide { .. }) {
                continue;
            }
            let keeper_hands =
                p.role == Role::Gk && pitch::in_penalty_area(p.pos, self.ends[side_of(i)]);
            let reach = if let Act::Dive { .. } = p.act {
                HANDS_REACH + Fx::milli(300)
            } else if keeper_hands {
                HANDS_REACH
            } else {
                REACH
            };
            let max_h = if keeper_hands { HANDS_H } else { HEAD_H };
            if h > max_h {
                continue;
            }
            let d = (bp - p.pos).len_sq_raw();
            if d < reach.0 as i64 * reach.0 as i64 && best.is_none_or(|(_, bd)| d < bd) {
                best = Some((i, d));
            }
        }
        let Some((i, _)) = best else {
            return;
        };
        let s = side_of(i);
        let keeper_hands = self.players[i].role == Role::Gk
            && pitch::in_penalty_area(self.players[i].pos, self.ends[s]);
        let speed = self.ball.vel.len();
        if keeper_hands {
            // Catch, or parry a hard shot.
            let skill = self.attr(i, team::DEFENSE);
            let hold = Fx::milli(1_050)
                - (speed - Fx::int(14))
                    .max(Fx::ZERO)
                    .mul(Fx::milli(40))
                    .mul(Fx::milli(1_300) - skill);
            if !self.touch(i) {
                return;
            }
            if self.rng.chance(hold) {
                self.owner = Some(i as u8);
                self.players[i].act = Act::Hold { t: 70 };
                self.players[i].vel = V2::ZERO;
            } else {
                // Parried: out into play, or round the post for a corner.
                let back = self.rng.chance(Fx::milli(400));
                let x = if back {
                    self.ends[s].sign()
                } else {
                    -self.ends[s].sign()
                };
                let y = if back {
                    self.ball.pos.z.signum().mul(Fx::milli(2_500))
                        + self.rng.signed().mul(Fx::milli(300))
                } else {
                    self.rng.signed()
                };
                let out = V2::new(x, y).norm();
                self.ball.vel = V3::from_ground(out.scale(speed.mul(Fx::milli(350))), Fx::int(2));
                self.players[i].cool = 20;
            }
            return;
        }
        if h > CHEST_H {
            // A header: attackers in the box go for goal, defenders clear,
            // midfield headers find a teammate.
            if !self.touch(i) {
                return;
            }
            let attack = self.attacks(s);
            let p = self.players[i].pos;
            let own_half = self.ax(s, p) < Fx::ZERO;
            let plan = if pitch::in_penalty_area(p, attack) {
                let aim = V2::new(attack.goal_x(), self.rng.signed().mul(pitch::GOAL_HALF_W));
                self.shot = Some(s as u8);
                self.shot_tick = self.tick;
                self.stats[s].shots += 1;
                self.log(EventKind::Shot, s, i as u8);
                ball::launch((aim - p).norm(), Fx::int(13), Fx::milli(80))
            } else if own_half && side_of(self.last_touch as usize) != s {
                let away =
                    V2::new(-self.ends[s].sign(), self.rng.signed().mul(Fx::milli(500))).norm();
                ball::launch(away, Fx::int(14), Fx::milli(450))
            } else {
                // Nod it on to the nearest teammate ahead.
                let fwd = attack.sign();
                let mut best: Option<(usize, Fx)> = None;
                for j in s * SIDE..s * SIDE + SIDE {
                    let q = &self.players[j];
                    if j == i || q.off || q.role == Role::Gk {
                        continue;
                    }
                    let d = q.pos - p;
                    let l = d.len();
                    if l > Fx::int(3)
                        && l < Fx::int(16)
                        && d.x.mul(fwd) > -Fx::int(2)
                        && best.is_none_or(|(_, b)| l < b)
                    {
                        best = Some((j, l));
                    }
                }
                match best {
                    Some((j, l)) => {
                        self.receiver = Some(j as u8);
                        self.pass = Some((s as u8, i as u8));
                        self.stats[s].passes += 1;
                        let dir = (self.players[j].pos - p).norm();
                        ball::launch(dir, Fx::int(5) + l.mul(Fx::milli(450)), Fx::milli(300))
                    }
                    None => ball::launch(V2::new(fwd, Fx::ZERO), Fx::int(9), Fx::milli(350)),
                }
            };
            self.ball.vel = plan;
            self.ball.spin = Fx::ZERO;
            self.players[i].cool = 10;
            self.offside = Some(OffsideWatch {
                side: s as u8,
                mask: self.offside_mask(s, i),
            });
            return;
        }
        // Feet (or chest): control, unless it comes too hot for his touch.
        let skill = self.attr(i, team::DRIBBLE);
        let mut control = Fx::ONE
            - (speed - Fx::int(11))
                .max(Fx::ZERO)
                .mul(Fx::milli(45))
                .mul(Fx::milli(1_200) - skill);
        if h > FOOT_H {
            control = control.mul(Fx::milli(850));
        }
        if !self.touch(i) {
            return;
        }
        if self.rng.chance(control) {
            self.owner = Some(i as u8);
            self.owned_since = self.tick;
            self.ball.pos.y = BALL_R;
            self.ball.vel = V3::from_ground(self.players[i].vel, Fx::ZERO);
            self.ball.spin = Fx::ZERO;
        } else {
            // Bounced off him.
            let v = self.ball.vel;
            self.ball.vel = V3::new(
                -v.x.mul(Fx::milli(300)),
                Fx::milli(1_500),
                -v.z.mul(Fx::milli(300)),
            );
            self.players[i].cool = 14;
        }
    }

    // --------------------------------------------------------- referee

    /// Linesmen follow the offside line on their half; the referee keeps
    /// a diagonal near the ball.
    fn move_officials(&mut self) {
        let b = self.ball.pos.ground();
        let target = V2::new(
            b.x - Fx::int(10).mul(b.y.signum() + Fx::milli(1)),
            b.y.mul(Fx::milli(400)) - Fx::int(8),
        );
        let r = &mut self.officials[0];
        let d = target - *r;
        *r += d.clamp_len(Fx::milli(6_000).mul(DT));
        // Linesman 1 on the -y touchline covers the half at -x, linesman 2
        // the half at +x (the usual diagonal).
        for (k, sign) in [(1usize, -Fx::ONE), (2usize, Fx::ONE)] {
            let mut deepest = Fx::ZERO;
            for p in &self.players {
                if !p.off && p.role != Role::Gk && p.pos.x.mul(sign) > deepest.mul(sign) {
                    deepest = p.pos.x;
                }
            }
            let line = if b.x.mul(sign) > deepest.mul(sign) {
                b.x
            } else {
                deepest
            };
            let line = if line.mul(sign) < Fx::ZERO {
                Fx::ZERO
            } else {
                line
            };
            let want = V2::new(
                line.clamp(-HALF_LEN, HALF_LEN),
                sign.mul(HALF_WID + Fx::milli(1_500)),
            );
            let lm = &mut self.officials[k];
            let d = want - *lm;
            *lm += d.clamp_len(Fx::milli(7_000).mul(DT));
        }
    }

    fn referee(&mut self) {
        // Nothing beyond what the touches and lines decide.
    }

    fn whistle_end_of_half(&mut self) {
        self.owner = None;
        self.phase = Phase::Dead { t: 30 };
    }

    fn end_half(&mut self) {
        if self.half == 1 {
            self.log(EventKind::HalfTime, 0, 0);
            self.phase = Phase::HalfTime { t: HALF_TIME_TICKS };
        } else {
            self.log(EventKind::FullTime, 0, 0);
            self.phase = Phase::FullTime;
            self.owner = None;
        }
    }

    fn second_half(&mut self) {
        self.half = 2;
        self.clock = 0;
        self.ends = [self.ends[0].other(), self.ends[1].other()];
        // Players recover a little at the break.
        for p in self.players.iter_mut() {
            p.stamina = (p.stamina + Fx::milli(150)).min(Fx::ONE);
            p.act = Act::Free;
        }
        self.begin_restart(Restart {
            kind: RestartKind::KickOff,
            team: 1 - self.first_kick,
            spot: V2::ZERO,
        });
        for i in 0..N {
            self.players[i].pos = self.players[i].target;
            self.players[i].vel = V2::ZERO;
        }
    }

    fn celebrate(&mut self, side: usize) {
        let scorer = self.last_touch as usize;
        let corner = V2::new(
            self.attacks(side).goal_x().mul(Fx::milli(800)),
            HALF_WID
                .mul(Fx::milli(800))
                .mul(if self.players[scorer].pos.y < Fx::ZERO {
                    -Fx::ONE
                } else {
                    Fx::ONE
                }),
        );
        for i in 0..N {
            if self.players[i].off {
                continue;
            }
            self.players[i].target = if side_of(i) == side {
                if i == scorer || side_of(scorer) != side {
                    corner
                } else {
                    // Teammates run to the scorer.
                    let k = Fx::int((i % SIDE) as i32 - 5).mul(Fx::milli(500));
                    self.players[scorer].pos + V2::new(k, Fx::milli(800))
                }
            } else {
                // The others trudge back toward their own half.
                let home = super::ai::kickoff_spot(self, i, false);
                self.players[i].pos + (home - self.players[i].pos).scale(Fx::milli(30))
            };
        }
    }

    // ------------------------------------------------------ set pieces

    /// Start walking everyone into place for `r`.
    pub(super) fn begin_restart(&mut self, r: Restart) {
        self.restart = r;
        self.phase = Phase::Setup { t: 0 };
        self.owner = None;
        self.offside = None;
        self.pass = None;
        self.shot = None;
        self.ball = Ball::at_rest(r.spot);
        if r.kind == RestartKind::KickOff
            && self.half == 1
            && self.clock == 0
            && self.score == [0, 0]
        {
            self.first_kick = r.team;
        }
        self.taker = super::ai::pick_taker(self, r);
        for i in 0..N {
            if !self.players[i].off {
                self.players[i].target = super::ai::restart_spot(self, i, r);
                if matches!(
                    self.players[i].act,
                    Act::Celebrate | Act::Kick { .. } | Act::Tackle { .. } | Act::Dive { .. }
                ) {
                    self.players[i].act = Act::Free;
                }
            }
        }
        if self.humans[r.team as usize] {
            self.active[r.team as usize] = self.taker;
        }
        if r.kind == RestartKind::KickOff {
            self.log(EventKind::KickOff, r.team as usize, self.taker);
        }
    }

    fn setup_players(&mut self) {
        for i in 0..N {
            if self.players[i].off {
                continue;
            }
            let d = self.players[i].target - self.players[i].pos;
            let (dir, len) = d.norm_len();
            // Brisk: set pieces shouldn't drag.
            let cmd = Cmd::go(dir.scale(len.min(Fx::int(2)).div(Fx::int(2))), true);
            self.move_one(i, &cmd);
            let p = &mut self.players[i];
            if len < Fx::milli(300) {
                p.pos = p.target;
                p.vel = V2::ZERO;
            }
            if matches!(p.act, Act::Down { .. }) && len > Fx::ZERO {
                p.act = Act::Free;
            }
        }
        // Faster than legs: after a second, glide the stragglers.
        if let Phase::Setup { t } = self.phase
            && t > 60
        {
            for p in self.players.iter_mut() {
                if !p.off {
                    p.pos += (p.target - p.pos).scale(Fx::milli(120));
                }
            }
        }
    }

    /// The taker stands behind the ball facing up the pitch; the ball is
    /// his.
    fn snap_taker(&mut self) {
        let k = self.taker as usize;
        let s = side_of(k);
        let goal = self.attacks(s).goal_center();
        let spot = self.restart.spot;
        let face = match self.restart.kind {
            RestartKind::ThrowIn => V2::new(Fx::ZERO, -spot.y.signum()),
            RestartKind::Corner => {
                (V2::new(spot.x - goal.x.mul(Fx::milli(220)), Fx::ZERO) - spot).norm()
            }
            RestartKind::KickOff => V2::new(self.attacks(s).sign(), Fx::ZERO),
            _ => (goal - spot).norm(),
        };
        let face = if face == V2::ZERO {
            V2::new(Fx::ONE, Fx::ZERO)
        } else {
            face
        };
        let p = &mut self.players[k];
        p.face = face;
        p.pos = spot
            - face.scale(Fx::milli(if self.restart.kind == RestartKind::ThrowIn {
                300
            } else {
                600
            }));
        p.vel = V2::ZERO;
        p.act = Act::Free;
        p.cool = 0;
        self.owner = Some(self.taker);
        self.owned_since = self.tick;
    }

    fn hold_restart_positions(&mut self) {
        for i in 0..N {
            let p = &mut self.players[i];
            if p.off || i == self.taker as usize {
                continue;
            }
            p.vel = p.vel.scale(Fx::milli(800));
            p.pos += p.vel.scale(DT);
        }
    }

    /// Human pad at a restart (or with the ball): the kick it asks for.
    fn human_kick(&mut self, i: usize, pad: &Pad) -> Option<KickPlan> {
        super::ai::human_kick_plan(self, i, pad)
    }

    /// The AI side's restart kick.
    fn ai_restart_kick(&mut self) -> KickPlan {
        super::ai::restart_kick(self, self.taker as usize)
    }

    // ------------------------------------------------------------ humans

    fn human_tick(&mut self, s: usize, pad: &Pad, cmds: &mut [Option<Cmd>; N]) {
        // Keep control on someone able.
        let a = self.active[s] as usize;
        if side_of(a) != s || self.players[a].off || self.players[a].role == Role::Gk {
            self.active[s] = super::ai::nearest_to_ball(self, s, None).unwrap_or(a as u8);
        }
        if pad.switch && self.owner.is_none_or(|o| side_of(o as usize) != s) {
            if let Some(n) = super::ai::switch_target(self, s) {
                self.active[s] = n;
            }
        } else if self.owner.is_some_and(|o| side_of(o as usize) != s) || self.owner.is_none() {
            // Semi-auto switch on defense: when the ball is far from the
            // active player and a teammate is much closer.
            let a = self.active[s] as usize;
            if let Some(n) = super::ai::nearest_to_ball(self, s, None) {
                let b = self.ball.pos.ground();
                let da = (self.players[a].pos - b).len();
                let dn = (self.players[n as usize].pos - b).len();
                if da > dn + Fx::int(12) && self.tick.is_multiple_of(30) {
                    self.active[s] = n;
                }
            }
        }
        let a = self.active[s] as usize;
        let mut cmd = Cmd::go(pad.mv, pad.sprint);
        if self.owner == Some(a as u8) {
            if let Some(plan) = super::ai::human_kick_plan(self, a, pad) {
                cmd.act = Some(CmdAct::Kick(plan));
            }
        } else if pad.slide {
            cmd.act = Some(CmdAct::Slide(if pad.mv == V2::ZERO {
                self.players[a].face
            } else {
                pad.mv
            }));
        } else if pad.tackle {
            cmd.act = Some(CmdAct::Tackle);
        }
        cmds[a] = Some(cmd);
    }
}

/// What a player is told to do this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Cmd {
    /// Desired direction × urgency (|mv| ≤ 1).
    pub mv: V2,
    pub sprint: bool,
    /// Face this way instead of the run direction.
    pub face: Option<V2>,
    pub act: Option<CmdAct>,
}

impl Cmd {
    pub fn go(mv: V2, sprint: bool) -> Cmd {
        Cmd {
            mv,
            sprint,
            face: None,
            act: None,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum CmdAct {
    Kick(KickPlan),
    Tackle,
    Slide(V2),
    Dive(V2),
}
