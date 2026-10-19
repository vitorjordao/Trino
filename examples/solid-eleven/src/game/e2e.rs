//! The game played end to end through its buttons only: menus, a match to
//! full time with a human side driven by a bot, the report, a career round
//! with a transfer and an edited player, the save and its reload, pause
//! and quit.

extern crate std;

use super::app::{Scene, SolidElevenGame};
use crate::sim::engine::{Phase, SIDE};
use crate::sim::league::Career;
use trino_core::{Audio, Button, Game, InputState, MusicId, Platform, SaveError, SoundId, Vec2};

struct Mute;
impl Audio for Mute {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// A platform with a 2 KB save medium (the N64's EEPROM size).
#[derive(Default)]
struct Cart {
    save: std::vec::Vec<u8>,
    writes: u32,
}

impl Platform for Cart {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn save_capacity(&self) -> usize {
        2_032
    }
    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        let n = self.save.len().min(buf.len());
        buf[..n].copy_from_slice(&self.save[..n]);
        Ok(n)
    }
    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        if data.len() > self.save_capacity() {
            return Err(SaveError::TooLarge {
                len: data.len(),
                capacity: self.save_capacity(),
            });
        }
        self.save = data.to_vec();
        self.writes += 1;
        Ok(())
    }
}

const DT: f32 = 1.0 / 30.0;

struct Driver {
    g: SolidElevenGame,
    cart: Cart,
}

impl Driver {
    fn new(cart: Cart) -> Driver {
        Driver {
            g: SolidElevenGame::new(Vec2::new(320.0, 240.0)),
            cart,
        }
    }

    fn frame(&mut self, input: &InputState) {
        self.g.update_with(&mut self.cart, input, &mut Mute, DT);
    }

    /// Press and release a button (two frames).
    fn press(&mut self, b: Button) {
        let mut s = InputState::default();
        s.set(b, true);
        self.frame(&s);
        self.frame(&InputState::default());
    }

    fn presses(&mut self, b: Button, n: usize) {
        for _ in 0..n {
            self.press(b);
        }
    }

    fn idle(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(&InputState::default());
        }
    }
}

/// A human playing side `s`: run at the ball, dribble at goal, shoot near
/// it, pass when pressed, tackle a carrier, switch when far.
fn bot(g: &SolidElevenGame, s: usize, frame: u32) -> InputState {
    let mut input = InputState::default();
    let Some(session) = &g.session else {
        return input;
    };
    let m = &session.m;
    let a = m.active[s] as usize;
    let me = m.players[a].pos;
    let ball = m.ball.pos.ground();
    let goal = m.attacks(s).goal_center();
    let to = |p: crate::sim::fx::V2| {
        let d = p - me;
        let (n, l) = d.norm_len();
        if l.0 == 0 {
            Vec2::ZERO
        } else {
            // Stick up = -z on the pitch.
            Vec2::new(n.x.to_f32(), -n.y.to_f32())
        }
    };
    match m.phase {
        Phase::Ready { .. } if m.restart.team as usize == s && m.taker as usize == a => {
            // Take restarts with a quick pass.
            if frame % 20 < 4 {
                input.set(Button::A, true);
            }
        }
        Phase::Play => {
            if m.owner == Some(a as u8) {
                input.stick = to(goal);
                input.set(Button::R, true);
                let dist = (goal - me).len().to_f32();
                // Charge the shot near goal; release it a few frames later.
                if dist < 24.0 && frame % 24 < 14 {
                    input.set(Button::B, true);
                } else if frame % 90 == 45 {
                    input.set(Button::A, true);
                }
            } else {
                input.stick = to(ball);
                input.set(Button::R, true);
                if m.owner.is_some_and(|o| (o as usize) / SIDE != s)
                    && (ball - me).len().to_f32() < 1.6
                    && frame.is_multiple_of(12)
                {
                    input.set(Button::A, true);
                }
                if (ball - me).len().to_f32() > 18.0 && frame.is_multiple_of(40) {
                    input.set(Button::L, true);
                }
            }
        }
        _ => {}
    }
    input
}

#[test]
fn a_quick_match_from_the_title_to_the_report_with_a_human_side() {
    let mut d = Driver::new(Cart::default());
    d.idle(3);
    assert_eq!(d.g.scene, Scene::Title);
    // Settings: halves of one minute.
    d.presses(Button::DpadDown, 4);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Settings);
    d.presses(Button::DpadDown, 2);
    d.presses(Button::DpadLeft, 2);
    assert_eq!(d.g.settings.half_mins, 1);
    d.press(Button::B);
    // Title > Play now.
    d.presses(Button::DpadUp, 4);
    assert_eq!(d.g.cur.title, 0);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Quick);
    // Home: Solar City (3), away: Violet Wanderers (6). (The away club
    // steps aside while the home one passes it.)
    d.presses(Button::DpadRight, 3);
    d.press(Button::DpadDown);
    while d.g.quick.away != 6 {
        d.press(Button::DpadRight);
    }
    assert_eq!((d.g.quick.home, d.g.quick.away), (3, 6));
    // The stadium (the third ground), the difficulty (hard), then the
    // Kick off button.
    d.presses(Button::DpadDown, 2);
    d.presses(Button::DpadRight, 2);
    assert_eq!(d.g.quick.stadium, 2);
    d.presses(Button::DpadDown, 3);
    let difficulty = d.g.settings.difficulty;
    d.press(Button::DpadRight);
    assert_eq!(d.g.settings.difficulty, (difficulty + 1) % 3);
    d.press(Button::DpadDown);
    assert_eq!(d.g.cur.quick, crate::game::app::QUICK_GO);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Match);
    assert_eq!(
        d.g.session.as_ref().unwrap().view.stadium,
        crate::render::Stadium::Park
    );
    // The bot plays the home side to full time.
    let mut frame = 0;
    while matches!(d.g.scene, Scene::Match | Scene::HalfTime) && frame < 30 * 60 * 6 {
        if d.g.scene == Scene::HalfTime {
            d.press(Button::A);
        }
        let input = bot(&d.g, 0, frame);
        d.frame(&input);
        frame += 1;
    }
    assert_eq!(
        d.g.scene,
        Scene::Results,
        "no full time after {frame} frames"
    );
    let rep = d.g.report.unwrap();
    assert_eq!(rep.clubs, [3, 6]);
    assert!(
        rep.stats[0].passes > 0,
        "the human side passed: {:?}",
        rep.stats[0]
    );
    assert!(rep.stats[0].possession > 0);
    std::println!(
        "human side: {:?}\nai side: {:?}\nscore {:?}",
        rep.stats[0],
        rep.stats[1],
        rep.score
    );
    assert!(!rep.career);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Title);
    // Settings were saved.
    assert!(d.cart.writes >= 1);
}

#[test]
fn a_career_round_trip_survives_the_save() {
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    // Title > Career > pick the 4th club.
    d.press(Button::DpadDown);
    d.press(Button::A);
    assert!(matches!(d.g.scene, Scene::PickClub { .. }));
    d.presses(Button::DpadDown, 3);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Club);
    assert!(d.g.career_on);
    assert_eq!(d.g.world.user, 3);
    // Simulate the first round.
    d.press(Button::DpadDown);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Results);
    assert_eq!(d.g.world.round, 1);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Club);
    // Market: buy the cheapest player on offer.
    d.presses(Button::DpadDown, 2);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Market);
    let (list, n) = d.g.market_list();
    let cheapest = (0..n)
        .min_by_key(|&k| d.g.world.buy_price(list[k] as usize))
        .unwrap();
    while d.g.cur.market != cheapest {
        d.press(Button::DpadDown);
    }
    let target = list[cheapest] as usize;
    let before = d.g.world.clubs[3].budget;
    d.press(Button::A);
    assert_eq!(d.g.world.club_of[target], 3, "bought");
    assert!(d.g.world.clubs[3].budget < before);
    d.press(Button::B);
    assert_eq!(d.g.scene, Scene::Club);
    // Squad (one up from Market) > edit the first player: shift points
    // from defense to speed.
    d.press(Button::DpadUp);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Teams);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Edit);
    let (i, _) = d.g.edit.unwrap();
    let speed = d.g.world.players[i].attrs[0];
    d.presses(Button::DpadDown, 5);
    d.presses(Button::DpadLeft, 3);
    d.presses(Button::DpadUp, 5);
    d.presses(Button::DpadRight, 5);
    assert_eq!(
        d.g.world.players[i].attrs[0],
        speed + 3,
        "only the freed points"
    );
    assert!(d.g.world.players[i].total() <= d.g.world.players[i].budget);
    // His look: the skin tone, the hair style and its color (rows 7-9).
    let look = d.g.world.players[i].appearance();
    d.presses(Button::DpadDown, 6);
    d.presses(Button::DpadRight, 2);
    d.press(Button::DpadDown);
    d.presses(Button::DpadRight, 2);
    d.press(Button::DpadDown);
    d.press(Button::DpadLeft);
    let edited = d.g.world.players[i].appearance();
    assert_eq!(edited.skin, (look.skin + 2) % 8);
    assert_eq!(edited.style, (look.style + 2) % 6);
    assert_eq!(edited.hair, (look.hair + 4) % 5);
    d.press(Button::A);
    d.press(Button::B);
    d.press(Button::B);
    assert_eq!(d.g.scene, Scene::Title);
    d.idle(2);
    let saved = d.g.world.clone();
    // A new game on the same cartridge resumes the career.
    let mut e = Driver::new(core::mem::take(&mut d.cart));
    e.idle(1);
    assert!(e.g.career_on);
    assert_eq!(e.g.world, saved);
    assert_eq!(
        e.g.world.players[i].appearance(),
        edited,
        "the look is saved"
    );
    assert_eq!(e.g.world, {
        let mut buf = [0u8; crate::sim::league::SAVE_MAX];
        let n = saved.save(e.g.settings.bytes(), &mut buf);
        Career::load(&buf[..n]).unwrap().0
    });
}

#[test]
fn quitting_a_career_match_counts_as_a_defeat() {
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    d.press(Button::DpadDown);
    d.press(Button::A);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Club);
    // Play the round's match, then pause and quit.
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Match);
    d.idle(30);
    d.press(Button::Start);
    assert_eq!(d.g.scene, Scene::Pause);
    // Resume, Substitute, Quit.
    d.presses(Button::DpadDown, 2);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Club);
    assert_eq!(d.g.world.round, 1);
    let row = d.g.world.table[0];
    assert_eq!((row.lost, row.ga), (1, 3));
}

#[test]
fn every_screen_draws_without_panicking_on_every_profile() {
    use crate::selftest::NullRenderer;
    for caps in [
        trino_core::Caps::N64,
        trino_core::Caps::N3DS,
        trino_core::Caps::PC,
    ] {
        let mut r = NullRenderer { caps };
        let mut d = Driver::new(Cart::default());
        d.idle(2);
        for scene in [
            Scene::Title,
            Scene::Quick,
            Scene::PickClub {
                then: super::app::After::Club,
            },
            Scene::Club,
            Scene::Table,
            Scene::Teams,
            Scene::Market,
            Scene::Settings,
            Scene::Confirm,
        ] {
            d.g.scene = scene;
            d.g.render(&mut r);
        }
        d.g.start_exhibition(0, 1, 1, [true, false], crate::render::Sky::Night, 30);
        d.idle(60);
        d.g.render(&mut r);
        d.g.scene = Scene::Pause;
        d.g.render(&mut r);
    }
}

// ------------------------------------------------------------- controls

/// The PC host as the controls page sees it: labels for every button and
/// a record of what the page asked for.
#[derive(Default)]
struct PcHost {
    cart: Cart,
    asked: std::vec::Vec<(Button, trino_core::BindDevice)>,
    resets: u32,
    capturing: Option<(Button, trino_core::BindDevice)>,
}

impl Platform for PcHost {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn save_capacity(&self) -> usize {
        self.cart.save_capacity()
    }
    fn save_read(&mut self, buf: &mut [u8]) -> Result<usize, SaveError> {
        self.cart.save_read(buf)
    }
    fn save_write(&mut self, data: &[u8]) -> Result<(), SaveError> {
        self.cart.save_write(data)
    }
    fn input_info(&self) -> Option<trino_core::InputInfo> {
        Some(trino_core::InputInfo {
            gamepad: trino_core::GamepadStatus::Connected,
            capturing: self.capturing,
        })
    }
    fn gamepad_name(&self) -> Option<&str> {
        Some("Test Pad")
    }
    fn binding_label(&self, b: Button, d: trino_core::BindDevice) -> Option<&str> {
        Some(match (d, b) {
            (trino_core::BindDevice::Keyboard, Button::A) => "Z",
            (trino_core::BindDevice::Keyboard, Button::Start) => "Enter",
            (trino_core::BindDevice::Gamepad, Button::L) => "LB/LT",
            _ => return None,
        })
    }
    fn rebind(&mut self, b: Button, d: trino_core::BindDevice) {
        self.asked.push((b, d));
        self.capturing = Some((b, d));
    }
    fn cancel_rebind(&mut self) {
        self.capturing = None;
    }
    fn reset_bindings(&mut self) {
        self.resets += 1;
    }
}

fn host_frame(g: &mut SolidElevenGame, host: &mut PcHost, input: &InputState) {
    g.update_with(host, input, &mut Mute, DT);
}

fn host_press(g: &mut SolidElevenGame, host: &mut PcHost, b: Button) {
    let mut s = InputState::default();
    s.set(b, true);
    host_frame(g, host, &s);
    host_frame(g, host, &InputState::default());
}

/// Title → settings → the controls row.
fn open_controls(d: &mut Driver) {
    d.idle(2);
    d.presses(Button::DpadDown, 4);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Settings);
    let rows = SolidElevenGame::settings_rows().len();
    d.presses(Button::DpadDown, rows - 1);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Controls);
}

#[test]
fn the_controls_page_rebinds_a_match_button_and_saves_it() {
    use crate::game::input::{Action, Bindings};
    let mut d = Driver::new(Cart::default());
    open_controls(&mut d);
    // Pass is the first row: change it, then press Y.
    d.press(Button::A);
    assert!(d.g.capture.is_some(), "waiting for the new button");
    d.press(Button::Y);
    assert!(d.g.capture.is_none());
    let b = d.g.settings.bindings;
    assert_eq!(b.of(Action::Pass), Button::Y);
    assert_eq!(
        b.of(Action::Keeper),
        Button::A,
        "the keeper takes pass's old button"
    );
    // Saved: a new game on the same cart loads it.
    d.press(Button::B);
    d.press(Button::B);
    d.idle(4);
    let cart = core::mem::take(&mut d.cart);
    let mut again = Driver::new(cart);
    again.idle(2);
    assert_eq!(again.g.settings.bindings.of(Action::Pass), Button::Y);
    // A match uses them: Y passes, A takes the keeper.
    again
        .g
        .start_exhibition(0, 1, 3, [true, false], crate::render::Sky::Day, 60);
    let s = again.g.session.as_ref().unwrap();
    assert_eq!(s.controls.bindings.of(Action::Pass), Button::Y);
    // Reset to the defaults (the last row).
    let mut d = again;
    d.g.scene = crate::game::app::Scene::Settings;
    d.g.cur.settings = 0;
    let rows = SolidElevenGame::settings_rows().len();
    d.presses(Button::DpadDown, rows - 1);
    d.press(Button::A);
    d.presses(Button::DpadUp, 1);
    d.press(Button::A);
    assert_eq!(d.g.settings.bindings, Bindings::DEFAULT);
}

#[test]
fn a_capture_gives_up_after_its_time() {
    use crate::game::input::{Action, Bindings};
    let mut d = Driver::new(Cart::default());
    open_controls(&mut d);
    d.presses(Button::DpadDown, 5);
    d.press(Button::A);
    let waited = d.g.capture.expect("capturing");
    assert_eq!(waited.action, Action::Sprint);
    d.idle((crate::game::app::CAPTURE_SECS / DT) as usize + 2);
    assert!(d.g.capture.is_none());
    assert_eq!(d.g.settings.bindings, Bindings::DEFAULT, "nothing changed");
    // The D-pad moves: pressing it while capturing changes nothing.
    d.press(Button::A);
    d.press(Button::DpadUp);
    assert!(d.g.capture.is_some(), "still waiting");
}

#[test]
fn the_pc_tabs_ask_the_host_to_rebind_keys_and_pad_buttons() {
    let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
    let mut host = PcHost::default();
    host_frame(&mut g, &mut host, &InputState::default());
    assert!(g.peek.rebindable);
    assert_eq!(g.peek.keys[0].as_str(), "Z");
    assert_eq!(g.peek.keys[6].as_str(), "ENTER", "uppercased for the font");
    assert_eq!(g.control_tabs(), 3);
    g.scene = Scene::Controls;
    // Keyboard tab, row 0 (A).
    host_press(&mut g, &mut host, Button::R);
    assert_eq!(g.cur.ctl_tab, 1);
    host_press(&mut g, &mut host, Button::A);
    assert_eq!(host.asked, [(Button::A, trino_core::BindDevice::Keyboard)]);
    // While the host waits, B cancels it (and the page stays put).
    host_frame(&mut g, &mut host, &InputState::default());
    assert!(g.peek.capturing.is_some());
    host_press(&mut g, &mut host, Button::B);
    host_frame(&mut g, &mut host, &InputState::default());
    assert!(host.capturing.is_none());
    assert_eq!(g.scene, Scene::Controls);
    // Gamepad tab: L's row, then the reset row.
    host_press(&mut g, &mut host, Button::R);
    assert_eq!(g.cur.ctl_tab, 2);
    for _ in 0..4 {
        host_press(&mut g, &mut host, Button::DpadDown);
    }
    host_press(&mut g, &mut host, Button::A);
    assert_eq!(host.asked[1], (Button::L, trino_core::BindDevice::Gamepad));
    host.capturing = None;
    // Up from row 4 wraps to the last row (the reset).
    for _ in 0..5 {
        host_press(&mut g, &mut host, Button::DpadUp);
    }
    assert_eq!(g.cur.ctl_row + 1, g.control_rows());
    host_press(&mut g, &mut host, Button::A);
    assert_eq!(host.resets, 1, "the last row resets the host's bindings");
}

#[test]
fn the_keeper_and_graphics_settings_reach_the_match() {
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    d.g.scene = Scene::Settings;
    let rows = SolidElevenGame::settings_rows();
    let k = rows
        .iter()
        .position(|r| *r == crate::game::app::SettingRow::Keeper)
        .unwrap();
    d.g.cur.settings = k;
    d.press(Button::DpadRight);
    assert!(d.g.settings.keeper);
    if let Some(gfx) = rows
        .iter()
        .position(|r| *r == crate::game::app::SettingRow::Graphics)
    {
        d.g.cur.settings = gfx;
        d.press(Button::DpadRight);
        assert_eq!(d.g.settings.graphics, 1);
    }
    d.g.start_exhibition(0, 1, 3, [true, false], crate::render::Sky::Day, 60);
    let s = d.g.session.as_ref().unwrap();
    assert_eq!(s.m.keeper_ctl, [true, false]);
    assert_eq!(s.view.retro, d.g.settings.graphics == 1);
}

/// A PC host with display settings (the runner's): records what it is asked.
struct DisplayHost {
    info: trino_core::DisplayInfo,
    asked: std::vec::Vec<u8>,
}

impl Platform for DisplayHost {
    fn now_micros(&self) -> u64 {
        0
    }
    fn log(&self, _: &str) {}
    fn display(&self) -> Option<trino_core::DisplayInfo> {
        Some(self.info)
    }
    fn set_display(&mut self, s: trino_core::DisplaySettings) {
        self.asked.push(s.pixel_scale);
        self.info.settings = s;
    }
}

#[test]
fn the_graphics_setting_asks_the_pc_host_for_its_resolution() {
    let mut host = DisplayHost {
        info: trino_core::DisplayInfo {
            settings: trino_core::DisplaySettings::DEFAULT,
            output: (1920, 1080),
            monitor: (1920, 1080),
            screen: (640, 360),
            pixel_scale: 3,
        },
        asked: std::vec::Vec::new(),
    };
    let mut g = SolidElevenGame::new(Vec2::new(640.0, 360.0));
    let idle = InputState::default();
    // Maximum (the default): the full output resolution, asked once.
    for _ in 0..3 {
        g.update_with(&mut host, &idle, &mut Mute, DT);
    }
    assert_eq!(host.asked, [1]);
    // Retro: the host's automatic (chunky) scale.
    g.settings.graphics = 1;
    g.update_with(&mut host, &idle, &mut Mute, DT);
    assert_eq!(host.asked, [1, 0]);
    // The window keeps its other settings.
    assert!(host.info.settings.fullscreen);
}

#[test]
fn a_rebound_pause_button_pauses_resumes_and_leaves_full_time_alone() {
    use crate::game::input::Action;
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    // Pause on Y: Y pauses and Y resumes.
    d.g.settings.bindings.assign(Action::Pause, Button::Y);
    d.g.start_exhibition(0, 1, 3, [true, false], crate::render::Sky::Day, 60);
    d.idle(5);
    d.press(Button::Y);
    assert_eq!(d.g.scene, Scene::Pause);
    d.press(Button::Y);
    assert_eq!(d.g.scene, Scene::Match);
    // Pause on A: at full time A confirms the result, it doesn't pause.
    d.g.settings.bindings.assign(Action::Pause, Button::A);
    d.g.start_exhibition(0, 1, 3, [true, false], crate::render::Sky::Day, 60);
    d.idle(5);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Pause, "A pauses during play");
    d.press(Button::A); // "resume", the first row
    assert_eq!(d.g.scene, Scene::Match);
    d.g.session.as_mut().unwrap().m.phase = Phase::FullTime;
    d.idle(60);
    d.press(Button::A);
    assert!(
        !matches!(d.g.scene, Scene::Match | Scene::Pause),
        "A at full time finishes the match ({:?})",
        d.g.scene
    );
}

#[test]
fn the_controls_page_survives_the_host_losing_its_tabs() {
    let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
    let mut host = PcHost::default();
    host_frame(&mut g, &mut host, &InputState::default());
    g.scene = Scene::Controls;
    // The pad tab's last row (the reset).
    host_press(&mut g, &mut host, Button::R);
    host_press(&mut g, &mut host, Button::R);
    host_press(&mut g, &mut host, Button::DpadUp);
    assert_eq!(g.cur.ctl_row + 1, g.control_rows());
    // Now a host without rebinding: one tab of 8 rows. Confirm picks a row
    // that exists.
    let mut cart = Cart::default();
    let mut press = |g: &mut SolidElevenGame, b: Button| {
        let mut s = InputState::default();
        s.set(b, true);
        g.update_with(&mut cart, &s, &mut Mute, DT);
        g.update_with(&mut cart, &InputState::default(), &mut Mute, DT);
    };
    press(&mut g, Button::A);
    assert_eq!(g.control_tabs(), 1);
    assert!(g.cur.ctl_row < g.control_rows());
}

/// A tap on a hair color swatch in the player editor picks that color
/// (the swatches' boxes come before their row's, so the row doesn't take
/// the tap).
#[test]
fn tapping_a_hair_swatch_picks_that_color() {
    use crate::selftest::NullRenderer;
    use trino_core::{Game, PointerButton};
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    // Title > Customize > the first player.
    d.presses(Button::DpadDown, 2);
    d.press(Button::A);
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Edit);
    let (i, _) = d.g.edit.unwrap();
    let mut r = NullRenderer {
        caps: trino_core::Caps::N3DS,
    };
    for k in [3u8, 0, 4] {
        d.g.render(&mut r);
        // The swatch's center on the 320x240 canvas (the screen here).
        let at = Vec2::new(221.0 + k as f32 * 17.5 + 7.5, 194.0);
        let mut down = InputState::default();
        down.pointer = Some(at);
        down.set_pointer_button(PointerButton::Primary, true);
        d.frame(&down);
        d.idle(1);
        assert_eq!(d.g.world.players[i].hair, k, "tapped swatch {k}");
        assert_eq!(d.g.cur.edit_attr, crate::sim::team::ATTRS + 2);
    }
}

/// Tapping the `<` back button in the screen header goes back to the parent screen.
#[test]
fn tapping_back_button_in_header_navigates_back() {
    use crate::selftest::NullRenderer;
    use trino_core::PointerButton;

    let mut d = Driver::new(Cart::default());
    d.idle(2);
    // Enter Quick Match setup
    d.press(Button::A);
    assert_eq!(d.g.scene, Scene::Quick);

    let mut r = NullRenderer {
        caps: trino_core::Caps::N3DS,
    };
    d.g.render(&mut r);

    // Tap the middle of the header's '<' (where the screen laid it out).
    let back =
        d.g.hits
            .iter()
            .find(|h| h.id == crate::ui::screens::HIT_BACK)
            .expect("the header's back arrow is a tap target")
            .rect;
    assert!(back.pos.y < 40.0, "in the header: {back:?}");
    let mut tap = InputState::default();
    tap.pointer = Some(Vec2::new(
        back.pos.x + back.size.x * 0.5,
        back.pos.y + back.size.y * 0.5,
    ));
    tap.set_pointer_button(PointerButton::Primary, true);
    d.frame(&tap);
    d.idle(1);

    assert_eq!(
        d.g.scene,
        Scene::Title,
        "tapping header back button returns to Title"
    );
}

/// Selecting 'Quit' in the pause menu (option 2) exits the match and clears session.
#[test]
fn pause_menu_quit_exits_match_to_title() {
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    d.press(Button::A); // Quick
    d.presses(Button::DpadDown, 6); // To start button
    d.press(Button::A); // Kick off
    assert_eq!(d.g.scene, Scene::Match);

    // Pause the match
    d.press(Button::Start);
    assert_eq!(d.g.scene, Scene::Pause);

    // Navigate to Option 2 (Quit) and confirm
    d.press(Button::DpadDown);
    d.press(Button::DpadDown);
    d.press(Button::A);

    assert_eq!(
        d.g.scene,
        Scene::Title,
        "tapping quit in pause menu exits match to Title"
    );
    assert!(d.g.session.is_none(), "session is cleared on quit");
}

/// The layout dynamically switches between Gamepad (pad) and Keyboard.
#[test]
fn dynamic_layout_switches_between_gamepad_and_keyboard() {
    let mut g = SolidElevenGame::new(Vec2::new(320.0, 240.0));
    let mut mute = Mute;

    // Use gamepad input: state.pad = true
    let mut pad_input = InputState::default();
    pad_input.pad = true;
    g.update(&pad_input, &mut mute, DT);
    assert!(g.using_pad, "active gamepad sets using_pad to true");
    assert!(g.pad_layout(), "gamepad layout is active");

    // Use keyboard input: key is pressed without pad
    let mut key_input = InputState::default();
    key_input.keys.set(trino_core::Key::Space, true);
    g.update(&key_input, &mut mute, DT);
    assert!(!g.using_pad, "keyboard press switches using_pad to false");
    assert!(!g.pad_layout(), "keyboard layout is active on PC");
}

/// Movement directions Left, Right, Up, Down match on-screen broadcast camera view.
#[test]
fn movement_left_right_up_down_matches_screen_space() {
    use crate::game::input::Controls;
    use crate::sim::fx::Fx;

    let mut c = Controls::default();
    let none = InputState::default();

    // Right: stick.x = +1.0 -> screen right is world -X
    let mut right = InputState::default();
    right.set(Button::DpadRight, true);
    let pad_r = c.pad(&right, &none, 1.0 / 60.0, false);
    assert_eq!(
        pad_r.mv.x,
        -Fx::ONE,
        "DpadRight moves player to screen right (-X)"
    );

    // Left: stick.x = -1.0 -> screen left is world +X
    let mut left = InputState::default();
    left.set(Button::DpadLeft, true);
    let pad_l = c.pad(&left, &none, 1.0 / 60.0, false);
    assert_eq!(
        pad_l.mv.x,
        Fx::ONE,
        "DpadLeft moves player to screen left (+X)"
    );

    // Up: stick.y = +1.0 -> runs away from camera into screen (-Y world Z)
    let mut up = InputState::default();
    up.set(Button::DpadUp, true);
    let pad_u = c.pad(&up, &none, 1.0 / 60.0, false);
    assert_eq!(pad_u.mv.y, -Fx::ONE, "DpadUp runs into the pitch (-Y)");

    // Down: stick.y = -1.0 -> runs toward camera (+Y world Z)
    let mut down = InputState::default();
    down.set(Button::DpadDown, true);
    let pad_d = c.pad(&down, &none, 1.0 / 60.0, false);
    assert_eq!(pad_d.mv.y, Fx::ONE, "DpadDown runs toward the camera (+Y)");
}

/// The pause menu (a UI screen since issue #58) draws the three rows its
/// logic has — before, it drew two, and the second ("QUIT MATCH") asked
/// for a substitution — in both languages, and the pad walks them.
#[test]
fn the_pause_menu_draws_and_walks_its_three_rows() {
    use crate::ui::i18n::Lang;
    struct Texts {
        caps: trino_core::Caps,
        texts: Vec<String>,
    }
    impl trino_core::Renderer for Texts {
        fn caps(&self) -> &trino_core::Caps {
            &self.caps
        }
        fn begin_frame(&mut self, _: trino_core::Color) {
            self.texts.clear();
        }
        fn draw_sprite(&mut self, _: trino_core::SpriteId, _: Vec2, _: &trino_core::SpriteParams) {}
        fn draw_model(
            &mut self,
            _: trino_core::ModelId,
            _: &trino_core::Transform3,
            _: trino_core::Material,
            _: &trino_core::ModelParams,
        ) {
        }
        fn end_frame(&mut self) {}
        fn draw_text(
            &mut self,
            _: trino_core::FontId,
            _: Vec2,
            text: &str,
            _: &trino_core::TextParams,
        ) {
            self.texts.push(text.to_string());
        }
        fn measure_text(&self, _: trino_core::FontId, t: &str, scale: f32) -> Vec2 {
            Vec2::new(t.chars().count() as f32 * 6.0 * scale, 11.0 * scale)
        }
    }
    let mut d = Driver::new(Cart::default());
    d.idle(2);
    d.g.start_exhibition(0, 1, 3, [true, false], crate::render::Sky::Day, 60);
    d.idle(5);
    d.press(Button::Start);
    assert_eq!(d.g.scene, Scene::Pause);
    for (lang, rows) in [
        (Lang::En, ["PAUSED", "RESUME", "SUBSTITUTE", "QUIT MATCH"]),
        (
            Lang::Pt,
            ["PAUSADO", "VOLTAR AO JOGO", "SUBSTITUIR", "ABANDONAR"],
        ),
    ] {
        d.g.settings.lang = lang;
        let mut r = Texts {
            caps: trino_core::Caps::N64,
            texts: Vec::new(),
        };
        d.g.render(&mut r);
        for row in rows {
            assert!(
                r.texts.iter().any(|t| t == row),
                "{lang:?}: {row} in {:?}",
                r.texts
            );
        }
    }
    assert_eq!(d.g.cur.pause, 0);
    d.press(Button::DpadDown);
    assert_eq!(d.g.cur.pause, 1, "substitute");
    d.press(Button::DpadDown);
    assert_eq!(d.g.cur.pause, 2, "quit");
    d.press(Button::B);
    assert_eq!(d.g.scene, Scene::Match, "B resumes");
}
