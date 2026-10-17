//! The game: which screen is up, the settings, the career (the club
//! database every mode reads, so edits and transfers show in quick
//! matches too), the match in progress, and the save.

use crate::game::input::{ACTIONS, ASSIGNABLE, Action, Bindings};
use crate::game::menu::{self, Hits, Nav, Repeat};
use crate::game::session::{Session, dress};
use crate::game::sound::{self, Sfx};
use crate::render::Sky;
use crate::render::anim::AnimState;
use crate::render::clips_gen::ClipName;
use crate::sim::engine::{N, Phase, SIDE, Setup, Stats};
use crate::sim::league::{Career, ROUNDS, SAVE_MAX, SETTINGS_LEN, TransferError};
use crate::sim::team::{self, ATTRS, CLUBS, FORMATIONS, PLAYERS, PlayerRec, TEAMS};
use crate::ui::i18n::{Lang, S};
use trino_core::{
    Audio, BindDevice, Button, Game, GamepadStatus, InputState, Platform, Renderer, TextBuf, Vec2,
};

/// The screens.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scene {
    Title,
    /// Quick-match setup.
    Quick,
    /// Choose the club to manage (then `then`).
    PickClub {
        then: After,
    },
    /// The career hub (club management).
    Club,
    /// The league table.
    Table,
    /// Squad and tactics of a club.
    Teams,
    /// Editing a player's attributes within his point budget.
    Edit,
    /// Buying and selling.
    Market,
    Settings,
    /// The settings' controls page: the match's buttons, and on the PC
    /// the keyboard's and the gamepad's, with the device's layout drawn.
    Controls,
    /// "Reset the save?"
    Confirm,
    Match,
    Pause,
    Results,
    /// The season's end: the champion lifts the trophy.
    Trophy,
}

/// Where a club pick leads.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum After {
    Club,
    Market,
}

/// Persistent settings (saved with the career).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Settings {
    pub lang: Lang,
    /// 0 easy, 1 normal, 2 hard.
    pub difficulty: u8,
    /// Minutes of play per half (real time).
    pub half_mins: u8,
    /// The human may take his keeper (and gets him on shots at his goal
    /// and while he holds the ball).
    pub keeper: bool,
    /// The match's buttons.
    pub bindings: Bindings,
    /// PC graphics: 0 = the PC's best (default), 1 = the console look.
    pub graphics: u8,
}

impl Settings {
    pub const DEFAULT: Settings = Settings {
        lang: Lang::En,
        difficulty: 1,
        half_mins: 3,
        keeper: false,
        bindings: Bindings::DEFAULT,
        graphics: 0,
    };

    /// The save's settings bytes: language, difficulty, half length, the
    /// career flag (byte 3, the app's), keeper, graphics, the bindings.
    pub fn bytes(&self) -> [u8; SETTINGS_LEN] {
        let mut b = [0u8; SETTINGS_LEN];
        b[0] = self.lang as u8;
        b[1] = self.difficulty;
        b[2] = self.half_mins;
        b[4] = self.keeper as u8;
        b[5] = self.graphics;
        b[6..6 + ACTIONS].copy_from_slice(&self.bindings.to_bytes());
        b
    }

    pub fn from_bytes(b: [u8; SETTINGS_LEN]) -> Settings {
        Settings {
            lang: if b[0] == 1 { Lang::Pt } else { Lang::En },
            difficulty: b[1].min(2),
            half_mins: b[2].clamp(1, 9),
            keeper: b[4] == 1,
            bindings: Bindings::from_bytes(&b[6..6 + ACTIONS]),
            graphics: b[5].min(1),
        }
    }
}

/// The quick-match choices.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QuickCfg {
    pub home: u8,
    pub away: u8,
    /// 0 = P1 plays home, 1 = P1 plays away, 2 = CPU v CPU.
    pub side: u8,
    pub sky: u8,
    pub rain: bool,
}

/// The last match's report.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Report {
    pub clubs: [u8; 2],
    pub score: [u8; 2],
    pub stats: [Stats; 2],
    /// Scorers: player database index and minute (up to 10).
    pub goals: [(u16, u8, u8); 10],
    pub n_goals: u8,
    pub career: bool,
    pub income: i32,
}

/// A message toast (transfers, saves).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Toast {
    pub text: S,
    pub good: bool,
    pub left: f32,
}

/// Menu cursors.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Cursors {
    pub title: usize,
    pub quick: usize,
    pub pick: usize,
    pub club: usize,
    /// The club shown in Teams.
    pub team_club: u8,
    pub squad: usize,
    pub squad_top: usize,
    pub edit_attr: usize,
    pub market: usize,
    pub market_top: usize,
    /// 0 price, 1 position, 2 overall, 3 club.
    pub market_sort: u8,
    /// Buying (false) or selling (true).
    pub market_sell: bool,
    pub settings: usize,
    pub pause: usize,
    pub confirm: usize,
    /// The controls page: tab (0 match, 1 keyboard, 2 gamepad), row, and
    /// the first row shown.
    pub ctl_tab: u8,
    pub ctl_row: usize,
    pub ctl_top: usize,
}

/// The controller the controls page draws: the console's own, or the PC's
/// keyboard and gamepad.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Device {
    Pc,
    N64,
    N3ds,
}

pub const DEVICE: Device = if cfg!(target_os = "none") {
    Device::N64
} else if cfg!(target_os = "horizon") {
    Device::N3ds
} else {
    Device::Pc
};

/// What the PC host says about its bindings (copied every frame by
/// `update_with`; empty on consoles and without a host).
#[derive(Clone, Copy, Debug, Default)]
pub struct InputPeek {
    /// The host can rebind keys and pad buttons.
    pub rebindable: bool,
    /// Each engine button's key and pad button(s), uppercase, in
    /// `Button::ALL` order ("Z", "ENTER", "LB/LT").
    pub keys: [TextBuf<14>; 16],
    pub pads: [TextBuf<14>; 16],
    /// 0 = no gamepad support, 1 = none connected, 2 = connected.
    pub gamepad: u8,
    pub pad_name: TextBuf<28>,
    /// A rebind waiting for its key or pad button.
    pub capturing: Option<(Button, BindDevice)>,
}

/// A request for the PC host, made by the controls page and sent after the
/// frame's update (the update doesn't see the platform).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum HostAsk {
    Rebind(Button, BindDevice),
    Cancel,
    Reset,
}

/// The match-button capture: the action waiting for a press, seconds left.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Capture {
    pub action: Action,
    pub left: f32,
}

/// How long the controls page waits for the new button.
pub const CAPTURE_SECS: f32 = 5.0;

#[derive(Clone)]
pub struct SolidElevenGame {
    pub screen: Vec2,
    pub scene: Scene,
    pub settings: Settings,
    /// The clubs and players every mode uses; the career when `career_on`.
    pub world: Career,
    pub career_on: bool,
    pub session: Option<Session>,
    pub quick: QuickCfg,
    pub cur: Cursors,
    pub report: Option<Report>,
    pub toast: Option<Toast>,
    /// The player being edited (database index) and his attributes before.
    pub edit: Option<(usize, PlayerRec)>,
    /// The match is a career round (its result goes to the table).
    in_career_match: bool,
    /// Seconds of menu time (animations).
    pub time: f32,
    /// The menus' 3D players (title huddle, the showcased player).
    pub menu_anims: [AnimState; 6],
    /// Yaw of the showcased player on the pedestal.
    pub spin: f32,
    pub hits: Hits,
    repeat: Repeat,
    prev: InputState,
    /// Waiting for a save to be written / have we read the save yet.
    save_dirty: bool,
    loaded: bool,
    music: Option<bool>,
    /// Seconds since full time (the report follows).
    after_full_time: f32,
    /// A substitution asked for from the pause menu.
    sub_asked: bool,
    /// The PC host's bindings (the controls page).
    pub peek: InputPeek,
    /// For the host after this frame.
    pub host_ask: Option<HostAsk>,
    /// The controls page waiting for a match button.
    pub capture: Option<Capture>,
    /// The controller the controls page draws ([`DEVICE`]; tests preview
    /// the others).
    pub device: Device,
    /// The pixel scale last asked of the PC host for the graphics setting.
    asked_scale: Option<u8>,
}

impl SolidElevenGame {
    pub fn new(screen: Vec2) -> SolidElevenGame {
        SolidElevenGame {
            screen,
            scene: Scene::Title,
            settings: Settings::DEFAULT,
            world: Career::new(0, 0x5011_D11E),
            career_on: false,
            session: None,
            quick: QuickCfg {
                home: 0,
                away: 1,
                side: 0,
                sky: 0,
                rain: false,
            },
            cur: Cursors {
                team_club: 0,
                ..Cursors::default()
            },
            report: None,
            toast: None,
            edit: None,
            in_career_match: false,
            time: 0.0,
            // The captain lifts the trophy, four teammates celebrate.
            menu_anims: core::array::from_fn(|i| {
                let clip = if i == 0 {
                    ClipName::TrophyLift
                } else {
                    ClipName::Celebrate
                };
                let mut a = AnimState::new(clip);
                a.time = i as f32 * 0.21;
                a
            }),
            spin: 0.0,
            hits: Hits::default(),
            repeat: Repeat::default(),
            prev: InputState::default(),
            save_dirty: false,
            loaded: false,
            music: None,
            after_full_time: 0.0,
            sub_asked: false,
            peek: InputPeek::default(),
            host_ask: None,
            capture: None,
            device: DEVICE,
            asked_scale: None,
        }
    }

    /// Straight into a match between clubs `home` and `away` with their
    /// generated squads (tests, perf scenes, screenshots, the self-test).
    pub fn exhibition(
        screen: Vec2,
        home: usize,
        away: usize,
        seed: u32,
        humans: [bool; 2],
        sky: Sky,
    ) -> SolidElevenGame {
        let mut g = SolidElevenGame::new(screen);
        g.start_exhibition(home, away, seed, humans, sky, 180);
        g
    }

    /// Start a match on the generated squads (not the career's).
    pub fn start_exhibition(
        &mut self,
        home: usize,
        away: usize,
        seed: u32,
        humans: [bool; 2],
        sky: Sky,
        half_secs: u32,
    ) {
        let (mut setup, names, skins) = crate::quick_setup(home, away, seed, half_secs, humans);
        setup.keeper_ctl = [self.settings.keeper; 2];
        let d = dress([setup.sides[0].club, setup.sides[1].club], &skins);
        let mut session = Session::new(&setup, &d, names, sky);
        session.controls.bindings = self.settings.bindings;
        session.view.retro = self.settings.graphics == 1;
        self.session = Some(session);
        self.scene = Scene::Match;
        self.in_career_match = false;
    }

    /// Start a match from the world's squads.
    fn start_match(
        &mut self,
        home: u8,
        away: u8,
        humans: [bool; 2],
        sky: Sky,
        rain: bool,
        career: bool,
    ) {
        let seed = 0x5E11
            ^ (self.world.year as u32) << 8
            ^ (self.world.round as u32) << 16
            ^ home as u32
            ^ (away as u32) << 4
            ^ (self.time * 1000.0) as u32;
        let half_ticks = self.settings.half_mins as u32 * 60 * 60;
        let mut setup: Setup = self.world.setup(
            home,
            away,
            half_ticks,
            seed,
            humans,
            self.settings.difficulty,
            rain,
        );
        setup.keeper_ctl = [self.settings.keeper; 2];
        let mut names = [0u8; N];
        let mut skins = [0u8; N];
        for s in 0..2 {
            for k in 0..SIDE {
                let db = setup.sides[s].players[k].0 as usize;
                names[s * SIDE + k] = self.world.players[db].surname;
                skins[s * SIDE + k] = self.world.players[db].skin;
            }
        }
        let d = dress([home, away], &skins);
        let mut session = Session::new(&setup, &d, names, sky);
        session.controls.bindings = self.settings.bindings;
        session.view.retro = self.settings.graphics == 1;
        self.session = Some(session);
        self.scene = Scene::Match;
        self.in_career_match = career;
        self.after_full_time = 0.0;
    }

    /// The match's sim hash (the self-test compares it across targets).
    pub fn match_hash(&self) -> u32 {
        self.session.as_ref().map_or(0, |s| s.m.hash())
    }

    fn toast(&mut self, text: S, good: bool) {
        self.toast = Some(Toast {
            text,
            good,
            left: 1.8,
        });
    }

    /// One frame of the menus and the match.
    fn step(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        self.time += dt;
        if let Some(t) = &mut self.toast {
            t.left -= dt;
            if t.left <= 0.0 {
                self.toast = None;
            }
        }
        let prev = self.prev;
        let nav = self.repeat.nav(input, &prev, dt);
        let tap = self.hits.tapped(input, &prev);
        // Music: the theme in menus, the crowd in matches.
        let want_crowd = matches!(self.scene, Scene::Match | Scene::Pause);
        if self.music != Some(want_crowd) {
            self.music = Some(want_crowd);
            audio.play_music(
                if want_crowd {
                    sound::CROWD
                } else {
                    sound::THEME
                },
                true,
            );
        }
        for a in self.menu_anims.iter_mut() {
            a.advance(dt, 1.0);
        }
        self.spin += dt * 0.6;
        match self.scene {
            Scene::Match => self.match_frame(input, &prev, &nav, audio, dt),
            Scene::Pause => self.pause_frame(&nav, tap, audio),
            Scene::Controls => self.controls_frame(input, &prev, &nav, tap, audio, dt),
            _ => self.menu_frame(&nav, tap, audio),
        }
    }

    fn match_frame(
        &mut self,
        input: &InputState,
        prev: &InputState,
        nav: &Nav,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        let Some(s) = &mut self.session else {
            self.scene = Scene::Title;
            return;
        };
        // Pause sits on its own (rebindable) button.
        let pause = s.controls.bindings.of(Action::Pause);
        if input.just_pressed(prev, pause) && s.human.is_some() {
            self.scene = Scene::Pause;
            self.cur.pause = 0;
            sound::play(audio, Sfx::Click);
            return;
        }
        // Bottom-screen taps: a stamina tile picks the player, the
        // formation box cycles the formation.
        let mut touch = (0u8, 0u8, core::mem::take(&mut self.sub_asked));
        if let Some(id) = self.hits.tapped(input, prev) {
            if id == 101 {
                touch.2 = true;
            } else if id == 100 {
                let side = s.human.unwrap_or(0);
                touch.1 = (s.m.tactics[side].formation + 1) % FORMATIONS.len() as u8 + 1;
            } else if (id as usize) < SIDE {
                touch.0 = id + 1;
            }
        }
        s.update(input, prev, audio, dt, touch);
        if s.m.phase == Phase::FullTime {
            self.after_full_time += dt;
            if self.after_full_time > 3.0 || (nav.confirm && self.after_full_time > 0.8) {
                self.finish_match();
            }
        }
    }

    fn pause_frame(&mut self, nav: &Nav, tap: Option<u8>, audio: &mut dyn Audio) {
        menu::step(&mut self.cur.pause, 3, nav);
        if let Some(id) = tap {
            self.cur.pause = id as usize;
        }
        if nav.start || nav.back {
            self.scene = Scene::Match;
            return;
        }
        if nav.confirm || tap.is_some() {
            sound::play(audio, Sfx::Click);
            if self.cur.pause == 0 {
                self.scene = Scene::Match;
            } else if self.cur.pause == 1 {
                // The most tired player goes off at the next dead ball.
                self.sub_asked = true;
                self.scene = Scene::Match;
            } else {
                // Quit: a career match counts as a 3-0 defeat.
                if self.in_career_match
                    && let Some((h, _)) = self.world.user_fixture()
                {
                    let user_home = h == self.world.user;
                    self.world
                        .finish_round(Some(if user_home { (0, 3) } else { (3, 0) }));
                    self.save_dirty = true;
                }
                self.session = None;
                self.scene = if self.in_career_match {
                    Scene::Club
                } else {
                    Scene::Title
                };
            }
        }
    }

    /// Full time: the report, and the career's books.
    fn finish_match(&mut self) {
        let Some(s) = self.session.take() else {
            return;
        };
        let m = &s.m;
        let mut goals = [(0u16, 0u8, 0u8); 10];
        let mut n = 0;
        for (_, e) in m.events_since(0) {
            if let crate::sim::engine::EventKind::Goal { .. } = e.kind
                && n < 10
            {
                let p = &m.players[e.player as usize];
                let minute = (e.tick as u64 * 90 / (m.half_ticks as u64 * 2).max(1)) as u8;
                goals[n] = (p.db, e.side, minute.min(90));
                n += 1;
            }
        }
        let mut income = 0;
        if self.in_career_match {
            let before = self.world.clubs[self.world.user as usize].budget;
            self.world.finish_round(Some((m.score[0], m.score[1])));
            income = self.world.clubs[self.world.user as usize].budget - before;
            self.save_dirty = true;
        }
        self.report = Some(Report {
            clubs: m.clubs,
            score: m.score,
            stats: m.stats,
            goals,
            n_goals: n as u8,
            career: self.in_career_match,
            income,
        });
        self.scene = Scene::Results;
    }

    fn menu_frame(&mut self, nav: &Nav, tap: Option<u8>, audio: &mut dyn Audio) {
        let before = self.scene;
        let moved = nav.up || nav.down || nav.left || nav.right;
        match self.scene {
            Scene::Title => self.title(nav, tap),
            Scene::Quick => self.quick_setup(nav, tap),
            Scene::PickClub { then } => self.pick_club(nav, tap, then),
            Scene::Club => self.club(nav, tap),
            Scene::Table => {
                if nav.back || nav.confirm || nav.y || tap.is_some() {
                    self.scene = Scene::Club;
                }
            }
            Scene::Teams => self.teams(nav, tap),
            Scene::Edit => self.edit(nav, tap),
            Scene::Market => self.market(nav, tap),
            Scene::Settings => self.settings_menu(nav, tap),
            Scene::Confirm => {
                menu::step(&mut self.cur.confirm, 2, nav);
                if let Some(id) = tap {
                    self.cur.confirm = id as usize;
                }
                if nav.back {
                    self.scene = Scene::Title;
                } else if nav.confirm || tap.is_some() {
                    if self.cur.confirm == 1 {
                        let settings = self.settings;
                        self.world = Career::new(0, 0x5011_D11E);
                        self.career_on = false;
                        self.settings = settings;
                        self.save_dirty = true;
                        self.toast(S::Saved, true);
                    }
                    self.scene = Scene::Title;
                }
            }
            Scene::Results => {
                if nav.confirm || nav.back || tap.is_some() {
                    let career = self.report.is_some_and(|r| r.career);
                    // The last round: the champion's ceremony first.
                    let over = self.world.round as usize >= ROUNDS;
                    self.scene = if career && over {
                        Scene::Trophy
                    } else if career {
                        Scene::Club
                    } else {
                        Scene::Title
                    };
                }
            }
            Scene::Trophy => {
                if nav.confirm || nav.back || tap.is_some() {
                    self.scene = Scene::Club;
                }
            }
            Scene::Match | Scene::Pause | Scene::Controls => {}
        }
        if self.scene != before || nav.confirm {
            sound::play(audio, if nav.back { Sfx::Back } else { Sfx::Click });
        } else if moved {
            sound::play(audio, Sfx::Tap);
        }
    }

    fn title(&mut self, nav: &Nav, tap: Option<u8>) {
        menu::step(&mut self.cur.title, 6, nav);
        if let Some(id) = tap {
            self.cur.title = id as usize;
        }
        if !(nav.confirm || tap.is_some()) {
            return;
        }
        self.scene = match self.cur.title {
            0 => Scene::Quick,
            1 if self.career_on => Scene::Club,
            1 => Scene::PickClub { then: After::Club },
            2 => {
                self.cur.team_club = if self.career_on {
                    self.world.user
                } else {
                    self.quick.home
                };
                self.cur.squad = 0;
                self.cur.squad_top = 0;
                Scene::Teams
            }
            3 if self.career_on => Scene::Market,
            3 => Scene::PickClub {
                then: After::Market,
            },
            4 => Scene::Settings,
            _ => {
                self.cur.confirm = 0;
                Scene::Confirm
            }
        };
    }

    fn quick_setup(&mut self, nav: &Nav, tap: Option<u8>) {
        menu::step(&mut self.cur.quick, 6, nav);
        if let Some(id) = tap {
            self.cur.quick = id as usize;
        }
        let d = menu::lr(nav);
        let q = &mut self.quick;
        let wrap = |v: u8, n: u8, d: i32| ((v as i32 + d).rem_euclid(n as i32)) as u8;
        match self.cur.quick {
            0 => q.home = wrap(q.home, TEAMS as u8, d),
            1 => q.away = wrap(q.away, TEAMS as u8, d),
            2 => q.side = wrap(q.side, 3, d),
            3 => q.sky = wrap(q.sky, 3, d),
            4 if d != 0 => {
                q.rain = !q.rain;
            }
            _ => {}
        }
        if q.home == q.away {
            q.away = wrap(q.away, TEAMS as u8, if d < 0 { -1 } else { 1 });
        }
        if nav.back {
            self.scene = Scene::Title;
        } else if nav.confirm || nav.start || (tap.is_some() && self.cur.quick == 5) {
            let humans = match self.quick.side {
                0 => [true, false],
                1 => [false, true],
                _ => [false, false],
            };
            let sky = [Sky::Day, Sky::Dusk, Sky::Night][self.quick.sky as usize % 3];
            let q = self.quick;
            self.start_match(q.home, q.away, humans, sky, q.rain, false);
        }
    }

    fn pick_club(&mut self, nav: &Nav, tap: Option<u8>, then: After) {
        menu::step(&mut self.cur.pick, TEAMS, nav);
        if nav.left || nav.right {
            let d = menu::lr(nav);
            self.cur.pick = (self.cur.pick as i32 + d * 5).rem_euclid(TEAMS as i32) as usize;
        }
        if let Some(id) = tap {
            self.cur.pick = id as usize;
        }
        if nav.back {
            self.scene = Scene::Title;
        } else if nav.confirm || tap.is_some() {
            let settings = self.settings;
            self.world = Career::new(self.cur.pick as u8, 0x5011_D11E ^ self.time.to_bits());
            self.world.user = self.cur.pick as u8;
            self.settings = settings;
            self.career_on = true;
            self.save_dirty = true;
            self.scene = match then {
                After::Club => Scene::Club,
                After::Market => Scene::Market,
            };
        }
    }

    fn club(&mut self, nav: &Nav, tap: Option<u8>) {
        // 0 play, 1 simulate, 2 squad, 3 market, 4 table.
        menu::step(&mut self.cur.club, 5, nav);
        if let Some(id) = tap {
            self.cur.club = id as usize;
        }
        if nav.back {
            self.scene = Scene::Title;
            return;
        }
        if nav.y {
            self.scene = Scene::Table;
            return;
        }
        if !(nav.confirm || tap.is_some()) {
            return;
        }
        let season_over = self.world.round as usize >= ROUNDS;
        match self.cur.club {
            0 | 1 if season_over => {
                self.world.new_season();
                self.save_dirty = true;
            }
            0 => {
                if let Some((h, a)) = self.world.user_fixture() {
                    let humans = [h == self.world.user, a == self.world.user];
                    let sky = [Sky::Day, Sky::Dusk, Sky::Night][self.world.round as usize % 3];
                    let rain = self.world.round % 4 == 3;
                    self.start_match(h, a, humans, sky, rain, true);
                }
            }
            1 => {
                if let Some((h, a)) = self.world.user_fixture() {
                    let mut rng = crate::sim::fx::Rng::new(
                        self.world.seed ^ 0xABCD ^ self.world.round as u32,
                    );
                    let s = self.world.quick_score(h, a, &mut rng);
                    let before = self.world.clubs[self.world.user as usize].budget;
                    self.world.finish_round(Some(s));
                    self.report = Some(Report {
                        clubs: [h, a],
                        score: [s.0, s.1],
                        stats: [Stats::default(); 2],
                        goals: [(0, 0, 0); 10],
                        n_goals: 0,
                        career: true,
                        income: self.world.clubs[self.world.user as usize].budget - before,
                    });
                    self.save_dirty = true;
                    self.scene = Scene::Results;
                }
            }
            2 => {
                self.cur.team_club = self.world.user;
                self.cur.squad = 0;
                self.cur.squad_top = 0;
                self.scene = Scene::Teams;
            }
            3 => self.scene = Scene::Market,
            _ => self.scene = Scene::Table,
        }
    }

    /// The squad list Teams and the editor show (database indices).
    pub fn squad_list(&self, club: u8) -> ([u16; team::MAX_SQUAD], usize) {
        let mut out = [0u16; team::MAX_SQUAD];
        let mut n = 0;
        // The lineup first, then the bench.
        let xi = self.world.lineup(club);
        for &i in &xi {
            out[n] = i;
            n += 1;
        }
        for i in self.world.squad(club) {
            if !xi.contains(&(i as u16)) && n < out.len() {
                out[n] = i as u16;
                n += 1;
            }
        }
        (out, n)
    }

    fn teams(&mut self, nav: &Nav, tap: Option<u8>) {
        let (list, n) = self.squad_list(self.cur.team_club);
        menu::step(&mut self.cur.squad, n, nav);
        if let Some(id) = tap {
            self.cur.squad = id as usize;
        }
        menu::scroll(&mut self.cur.squad_top, self.cur.squad, 6);
        // L/R: another club (outside a career, any club can be edited).
        if (nav.l || nav.r) && !self.career_on {
            let d = if nav.r { 1 } else { TEAMS as u8 - 1 };
            self.cur.team_club = (self.cur.team_club + d) % TEAMS as u8;
            self.cur.squad = 0;
            self.cur.squad_top = 0;
        }
        if nav.x {
            let c = &mut self.world.clubs[self.cur.team_club as usize];
            c.formation = (c.formation + 1) % FORMATIONS.len() as u8;
            self.save_dirty = true;
        }
        if nav.back {
            self.scene = if self.career_on {
                Scene::Club
            } else {
                Scene::Title
            };
        } else if (nav.confirm || tap.is_some()) && n > 0 {
            let i = list[self.cur.squad] as usize;
            self.edit = Some((i, self.world.players[i]));
            self.cur.edit_attr = 0;
            self.scene = Scene::Edit;
        }
    }

    fn edit(&mut self, nav: &Nav, tap: Option<u8>) {
        let Some((i, backup)) = self.edit else {
            self.scene = Scene::Teams;
            return;
        };
        // Rows: six attributes, then the skin tone.
        menu::step(&mut self.cur.edit_attr, ATTRS + 1, nav);
        if let Some(id) = tap {
            self.cur.edit_attr = id as usize;
        }
        let d = menu::lr(nav);
        let p = &mut self.world.players[i];
        if d != 0 {
            if self.cur.edit_attr < ATTRS {
                if p.adjust(self.cur.edit_attr, d as i8).is_err() {
                    self.toast = Some(Toast {
                        text: S::PointsLeft,
                        good: false,
                        left: 1.0,
                    });
                }
            } else {
                p.skin = ((p.skin as i32 + d).rem_euclid(8)) as u8;
            }
        }
        if nav.x {
            // Random: redistribute the budget (deterministic per press).
            let mut rng = crate::sim::fx::Rng::new((self.time * 997.0) as u32 ^ i as u32);
            for a in 0..ATTRS {
                p.attrs[a] =
                    team::ATTR_MIN + rng.below((team::ATTR_MAX - team::ATTR_MIN) as u32 + 1) as u8;
            }
            team::balance(&mut p.attrs, p.budget);
        }
        if nav.back {
            self.world.players[i] = backup;
            self.edit = None;
            self.scene = Scene::Teams;
        } else if nav.confirm {
            self.edit = None;
            self.save_dirty = true;
            self.toast(S::Saved, true);
            self.scene = Scene::Teams;
        }
    }

    /// The market list: other clubs' players (buying) or the user's
    /// (selling), sorted by the chosen key.
    pub fn market_list(&self) -> ([u16; PLAYERS], usize) {
        let mut out = [0u16; PLAYERS];
        let mut n = 0;
        let user = self.world.user;
        for i in 0..PLAYERS {
            let mine = self.world.club_of[i] == user;
            if mine == self.cur.market_sell {
                out[n] = i as u16;
                n += 1;
            }
        }
        let w = &self.world;
        let key = |i: u16| -> (i64, u16) {
            let p = &w.players[i as usize];
            let k = match self.cur.market_sort {
                0 => -(w.buy_price(i as usize) as i64),
                1 => p.pos as i64 * 1000 - p.ovr() as i64,
                2 => -(p.ovr() as i64),
                _ => w.club_of[i as usize] as i64 * 1000 - p.ovr() as i64,
            };
            (k, i)
        };
        out[..n].sort_unstable_by_key(|&i| key(i));
        (out, n)
    }

    fn market(&mut self, nav: &Nav, tap: Option<u8>) {
        let (list, n) = self.market_list();
        menu::step(&mut self.cur.market, n.max(1), nav);
        if let Some(id) = tap {
            self.cur.market = id as usize;
        }
        menu::scroll(&mut self.cur.market_top, self.cur.market, 5);
        if nav.x {
            self.cur.market_sort = (self.cur.market_sort + 1) % 4;
            self.cur.market = 0;
            self.cur.market_top = 0;
        }
        if nav.y || nav.l || nav.r {
            self.cur.market_sell = !self.cur.market_sell;
            self.cur.market = 0;
            self.cur.market_top = 0;
        }
        if nav.back {
            self.scene = if self.career_on {
                Scene::Club
            } else {
                Scene::Title
            };
            return;
        }
        if nav.confirm && n > 0 {
            let i = list[self.cur.market.min(n - 1)] as usize;
            let result = if self.cur.market_sell {
                self.world.sell(i).map(|_| S::Sold)
            } else {
                self.world.buy(i).map(|_| S::Bought)
            };
            match result {
                Ok(s) => {
                    self.toast(s, true);
                    self.save_dirty = true;
                    self.cur.market = self.cur.market.min(n.saturating_sub(2));
                }
                Err(e) => {
                    let s = match e {
                        TransferError::NotEnoughCoins => S::NotEnough,
                        TransferError::SquadFull => S::SquadFull,
                        TransferError::SquadTooSmall
                        | TransferError::NoBuyer
                        | TransferError::OwnPlayer => S::SquadMin,
                    };
                    self.toast(s, false);
                }
            }
        }
    }

    /// The settings page's rows (the graphics row is the PC's).
    pub fn settings_rows() -> &'static [SettingRow] {
        if DEVICE == Device::Pc {
            &[
                SettingRow::Language,
                SettingRow::Difficulty,
                SettingRow::Length,
                SettingRow::Keeper,
                SettingRow::Graphics,
                SettingRow::Controls,
            ]
        } else {
            &[
                SettingRow::Language,
                SettingRow::Difficulty,
                SettingRow::Length,
                SettingRow::Keeper,
                SettingRow::Controls,
            ]
        }
    }

    fn settings_menu(&mut self, nav: &Nav, tap: Option<u8>) {
        let rows = Self::settings_rows();
        menu::step(&mut self.cur.settings, rows.len(), nav);
        if let Some(id) = tap {
            self.cur.settings = (id as usize).min(rows.len() - 1);
        }
        let row = rows[self.cur.settings.min(rows.len() - 1)];
        if row == SettingRow::Controls {
            if nav.confirm || tap.is_some() || nav.right {
                self.scene = Scene::Controls;
                self.cur.ctl_row = 0;
                self.cur.ctl_top = 0;
            }
        } else {
            let d = menu::lr(nav) + if nav.confirm || tap.is_some() { 1 } else { 0 };
            if d != 0 {
                let st = &mut self.settings;
                match row {
                    SettingRow::Language => {
                        st.lang = if st.lang == Lang::En {
                            Lang::Pt
                        } else {
                            Lang::En
                        }
                    }
                    SettingRow::Difficulty => {
                        st.difficulty = ((st.difficulty as i32 + d).rem_euclid(3)) as u8
                    }
                    SettingRow::Length => {
                        st.half_mins = ((st.half_mins as i32 - 1 + d).rem_euclid(6) + 1) as u8
                    }
                    SettingRow::Keeper => st.keeper = !st.keeper,
                    SettingRow::Graphics => st.graphics = 1 - st.graphics.min(1),
                    SettingRow::Controls => {}
                }
                self.save_dirty = true;
            }
        }
        if nav.back {
            self.scene = Scene::Title;
        }
    }

    /// The tabs the controls page has here (the PC adds its devices).
    pub fn control_tabs(&self) -> u8 {
        if self.peek.rebindable { 3 } else { 1 }
    }

    /// Rows of the controls page's current tab (the last resets it).
    pub fn control_rows(&self) -> usize {
        if self.cur.ctl_tab == 0 {
            ACTIONS + 1
        } else {
            Button::ALL.len() + 1
        }
    }

    fn controls_frame(
        &mut self,
        input: &InputState,
        prev: &InputState,
        nav: &Nav,
        tap: Option<u8>,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        // Waiting for the new match button: any assignable press takes it.
        if let Some(c) = &mut self.capture {
            c.left -= dt;
            let action = c.action;
            let pressed = ASSIGNABLE
                .into_iter()
                .find(|&b| input.just_pressed(prev, b));
            if let Some(b) = pressed {
                self.settings.bindings.assign(action, b);
                self.capture = None;
                self.save_dirty = true;
                sound::play(audio, Sfx::Click);
            } else if c.left <= 0.0 {
                self.capture = None;
                sound::play(audio, Sfx::Back);
            }
            return;
        }
        // The host is waiting for a key or pad button: B cancels.
        if self.peek.capturing.is_some() {
            if nav.back {
                self.host_ask = Some(HostAsk::Cancel);
            }
            return;
        }
        let tabs = self.control_tabs();
        let before = (self.cur.ctl_tab, self.cur.ctl_row);
        if nav.l || nav.r {
            let d = if nav.r { 1 } else { tabs - 1 };
            self.cur.ctl_tab = (self.cur.ctl_tab + d) % tabs;
            self.cur.ctl_row = 0;
            self.cur.ctl_top = 0;
        }
        self.cur.ctl_tab = self.cur.ctl_tab.min(tabs - 1);
        let rows = self.control_rows();
        menu::step(&mut self.cur.ctl_row, rows, nav);
        if let Some(id) = tap {
            if id >= 200 {
                self.cur.ctl_tab = (id - 200).min(tabs - 1);
                self.cur.ctl_row = 0;
                self.cur.ctl_top = 0;
            } else {
                self.cur.ctl_row = (id as usize).min(rows - 1);
            }
        }
        // Keep the selection in the visible window.
        let shown = CONTROL_ROWS_SHOWN;
        if self.cur.ctl_row < self.cur.ctl_top {
            self.cur.ctl_top = self.cur.ctl_row;
        } else if self.cur.ctl_row >= self.cur.ctl_top + shown {
            self.cur.ctl_top = self.cur.ctl_row + 1 - shown;
        }
        if (self.cur.ctl_tab, self.cur.ctl_row) != before {
            sound::play(audio, Sfx::Tap);
        }
        if nav.back {
            self.scene = Scene::Settings;
            sound::play(audio, Sfx::Back);
            return;
        }
        if !(nav.confirm || tap.is_some_and(|id| id < 200)) {
            return;
        }
        sound::play(audio, Sfx::Click);
        let reset = self.cur.ctl_row + 1 == rows;
        match (self.cur.ctl_tab, reset) {
            (0, true) => {
                self.settings.bindings = Bindings::DEFAULT;
                self.save_dirty = true;
                self.toast(S::Saved, true);
            }
            (0, false) => {
                self.capture = Some(Capture {
                    action: Action::ALL[self.cur.ctl_row],
                    left: CAPTURE_SECS,
                });
            }
            (_, true) => self.host_ask = Some(HostAsk::Reset),
            (tab, false) => {
                let device = if tab == 1 {
                    BindDevice::Keyboard
                } else {
                    BindDevice::Gamepad
                };
                self.host_ask = Some(HostAsk::Rebind(Button::ALL[self.cur.ctl_row], device));
            }
        }
    }

    /// The PC's graphics setting on its host: maximum renders at the
    /// output's full resolution (pixel scale 1), retro at the host's chunky
    /// automatic scale. Asked once per change (the host persists it).
    fn apply_graphics(&mut self, platform: &mut dyn Platform) {
        let Some(info) = platform.display() else {
            return;
        };
        let want = if self.settings.graphics == 0 { 1 } else { 0 };
        if self.asked_scale != Some(want) {
            self.asked_scale = Some(want);
            if info.settings.pixel_scale != want {
                platform.set_display(trino_core::DisplaySettings {
                    pixel_scale: want,
                    ..info.settings
                });
            }
        }
    }

    /// Copy the host's binding labels and rebind state (the PC's).
    fn peek_input(&mut self, platform: &dyn Platform) {
        let Some(info) = platform.input_info() else {
            self.peek = InputPeek::default();
            return;
        };
        let mut p = InputPeek {
            rebindable: true,
            capturing: info.capturing,
            gamepad: match info.gamepad {
                GamepadStatus::Unsupported => 0,
                GamepadStatus::Disconnected => 1,
                GamepadStatus::Connected => 2,
            },
            ..InputPeek::default()
        };
        for (k, b) in Button::ALL.into_iter().enumerate() {
            p.keys[k] = font_safe(platform.binding_label(b, BindDevice::Keyboard));
            p.pads[k] = font_safe(platform.binding_label(b, BindDevice::Gamepad));
        }
        p.pad_name = font_safe(platform.gamepad_name());
        self.peek = p;
    }

    /// Read the save on the first frame; write it when something changed
    /// (spread over frames: the N64's EEPROM takes ~7.5 ms per 8 bytes).
    fn persist(&mut self, platform: &mut dyn Platform) {
        if !self.loaded {
            self.loaded = true;
            let mut buf = [0u8; SAVE_MAX];
            if let Ok(n) = platform.save_read(&mut buf)
                && let Some((career, settings)) = Career::load(&buf[..n])
            {
                self.world = career;
                self.settings = Settings::from_bytes(settings);
                self.career_on = settings[3] == 1;
            }
        }
        // Never write during a match (the EEPROM steals frame time).
        if self.save_dirty && !matches!(self.scene, Scene::Match | Scene::Pause) {
            self.save_dirty = false;
            let mut buf = [0u8; SAVE_MAX];
            let mut st = self.settings.bytes();
            st[3] = self.career_on as u8;
            let n = self.world.save(st, &mut buf);
            let _ = platform.save_begin(&buf[..n]);
        }
        let _ = platform.save_step(2_000);
    }
}

impl Game for SolidElevenGame {
    fn update(&mut self, input: &InputState, audio: &mut dyn Audio, dt: f32) {
        self.step(input, audio, dt);
        self.prev = *input;
    }

    fn update_with(
        &mut self,
        platform: &mut dyn Platform,
        input: &InputState,
        audio: &mut dyn Audio,
        dt: f32,
    ) {
        self.persist(platform);
        self.peek_input(platform);
        self.apply_graphics(platform);
        self.update(input, audio, dt);
        if let Some(ask) = self.host_ask.take() {
            match ask {
                HostAsk::Rebind(b, d) => platform.rebind(b, d),
                HostAsk::Cancel => platform.cancel_rebind(),
                HostAsk::Reset => platform.reset_bindings(),
            }
        }
    }

    fn render(&mut self, renderer: &mut dyn Renderer) {
        crate::ui::screens::draw(self, renderer);
        renderer.end_frame();
    }

    fn resize(&mut self, screen: Vec2) {
        self.screen = screen;
    }
}

/// The settings page's rows.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SettingRow {
    Language,
    Difficulty,
    Length,
    Keeper,
    Graphics,
    Controls,
}

/// Rows the controls page shows at once.
pub const CONTROL_ROWS_SHOWN: usize = 9;

/// A host label in the UI font's characters (uppercase; others become '?').
pub fn font_safe<const N: usize>(s: Option<&str>) -> TextBuf<N> {
    use core::fmt::Write as _;
    let mut out = TextBuf::<N>::new();
    if let Some(s) = s {
        for c in s.chars() {
            let u = c.to_ascii_uppercase();
            let c = if crate::ui::FONT_CHARS.contains(u) {
                u
            } else {
                '?'
            };
            let _ = out.write_char(c);
        }
    }
    out
}

/// Names for the clubs' crests (three letters).
pub fn club_short(c: u8) -> &'static str {
    CLUBS[c as usize % TEAMS].short
}
