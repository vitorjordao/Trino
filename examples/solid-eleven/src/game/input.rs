//! Buttons to a [`Pad`]: the stick in pitch directions (the broadcast
//! camera looks north, so stick-up runs to -z), kicks charged while held and
//! played on release, tackles on press. Each [`Action`] sits on a button
//! the settings can change ([`Bindings`]); the defaults:
//!
//! | Action (button, PC key) | With the ball | Without it | Keeper |
//! |---|---|---|---|
//! | Pass (A, Z) | pass (hold = harder) | standing tackle | throw / dive |
//! | Shoot (B, X) | shoot (hold = power) | sliding tackle | punt |
//! | Through (X, C) | through ball / cross (hold = lofted) | — | long roll |
//! | Keeper (Y, V) | — | take the keeper (when allowed) | — |
//! | Switch (L, Q) | — | switch player | back to the field |
//! | Sprint (R, E) | sprint | sprint | sprint |
//! | Pause (Start, Enter) | pause | pause | pause |

use crate::sim::engine::Pad;
use crate::sim::fx::{Fx, V2};
use trino_core::{Button, InputState};

/// Seconds of holding for a full-power kick.
pub const FULL_CHARGE: f32 = 0.9;

/// What a button does in a match.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum Action {
    Pass,
    Shoot,
    Through,
    Keeper,
    Switch,
    Sprint,
    Pause,
}

pub const ACTIONS: usize = 7;

impl Action {
    pub const ALL: [Action; ACTIONS] = [
        Action::Pass,
        Action::Shoot,
        Action::Through,
        Action::Keeper,
        Action::Switch,
        Action::Sprint,
        Action::Pause,
    ];
}

/// The buttons an action may take: all but the D-pad (it moves the player).
pub const ASSIGNABLE: [Button; 12] = [
    Button::A,
    Button::B,
    Button::X,
    Button::Y,
    Button::L,
    Button::R,
    Button::L2,
    Button::R2,
    Button::L3,
    Button::R3,
    Button::Start,
    Button::Select,
];

/// On a PC gamepad one control presses two buttons (LT = L and L2, RT = R
/// and R2, the left stick's click = Select and L3: the engine's default pad
/// map). The specific button wins, so an action on L2 and another on L
/// never both fire from LT, and a capture of LT takes L2.
pub fn dealias(s: &InputState) -> InputState {
    let mut out = *s;
    if s.pad {
        for (specific, alias) in [
            (Button::L2, Button::L),
            (Button::R2, Button::R),
            (Button::L3, Button::Select),
        ] {
            if s.is_down(specific) {
                out.set(alias, false);
            }
        }
    }
    out
}

/// The button of each [`Action`] (saved with the settings). No button
/// does two things: [`assign`](Self::assign) swaps.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Bindings(pub [Button; ACTIONS]);

impl Bindings {
    pub const DEFAULT: Bindings = Bindings([
        Button::A,
        Button::B,
        Button::X,
        Button::Y,
        Button::L,
        Button::R,
        Button::Start,
    ]);

    pub fn of(&self, a: Action) -> Button {
        self.0[a as usize]
    }

    /// Give `a` the button `b`; the action that had `b` takes `a`'s old
    /// button. A button outside [`ASSIGNABLE`] is refused.
    pub fn assign(&mut self, a: Action, b: Button) -> bool {
        if !ASSIGNABLE.contains(&b) {
            return false;
        }
        let old = self.0[a as usize];
        if let Some(k) = self.0.iter().position(|&x| x == b) {
            self.0[k] = old;
        }
        self.0[a as usize] = b;
        true
    }

    pub fn to_bytes(&self) -> [u8; ACTIONS] {
        self.0.map(|b| b as u8)
    }

    /// From saved bytes; anything invalid (a stale or blank save, a
    /// button twice) gives the defaults.
    pub fn from_bytes(b: &[u8]) -> Bindings {
        let mut out = Bindings::DEFAULT;
        for (k, v) in b.iter().take(ACTIONS).enumerate() {
            match Button::ALL.get(*v as usize) {
                Some(&btn) if ASSIGNABLE.contains(&btn) => out.0[k] = btn,
                _ => return Bindings::DEFAULT,
            }
        }
        for i in 0..ACTIONS {
            if out.0[i + 1..].contains(&out.0[i]) {
                return Bindings::DEFAULT;
            }
        }
        out
    }
}

impl Default for Bindings {
    fn default() -> Bindings {
        Bindings::DEFAULT
    }
}

/// Kick actions, in `Controls::held` order.
const KICKS: [Action; 3] = [Action::Pass, Action::Shoot, Action::Through];

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Controls {
    /// Seconds each kick button has been held (with the ball).
    pub held: [f32; 3],
    /// The press started while the side could kick (else a release is not
    /// a kick: a slide pressed without the ball must not shoot later).
    armed: [bool; 3],
    /// The match's buttons (from the settings).
    pub bindings: Bindings,
}

impl Controls {
    /// The charge shown on the HUD (the strongest held kick, 0..1).
    pub fn charge(&self) -> f32 {
        let mut c = 0.0f32;
        for k in 0..3 {
            if self.armed[k] {
                c = c.max(self.held[k] / FULL_CHARGE);
            }
        }
        c.min(1.0)
    }

    /// This frame's pad. `can_kick`: the controlled player has the ball
    /// (or takes the restart).
    pub fn pad(&mut self, now: &InputState, prev: &InputState, dt: f32, can_kick: bool) -> Pad {
        let (now, prev) = (&dealias(now), &dealias(prev));
        let mut pad = Pad::default();
        let mut stick = now.stick;
        if now.is_down(Button::DpadLeft) {
            stick.x = -1.0;
        }
        if now.is_down(Button::DpadRight) {
            stick.x = 1.0;
        }
        if now.is_down(Button::DpadUp) {
            stick.y = 1.0;
        }
        if now.is_down(Button::DpadDown) {
            stick.y = -1.0;
        }
        let len = trino_core::math3d::sqrt(stick.x * stick.x + stick.y * stick.y);
        if len > 0.2 {
            let k = if len > 1.0 { 1.0 / len } else { 1.0 };
            pad.mv = V2::new(Fx::from_f32(-stick.x * k), Fx::from_f32(-stick.y * k));
        }
        let bind = self.bindings;
        let sprint = bind.of(Action::Sprint);
        // An analog right trigger sprints too when sprint sits on R or R2.
        let analog = matches!(sprint, Button::R | Button::R2) && now.triggers.y > 0.5;
        pad.sprint = now.is_down(sprint) || analog;
        pad.switch = now.just_pressed(prev, bind.of(Action::Switch));
        pad.keeper = now.just_pressed(prev, bind.of(Action::Keeper));
        for (k, act) in KICKS.into_iter().enumerate() {
            let b = bind.of(act);
            if now.just_pressed(prev, b) {
                self.armed[k] = can_kick;
                self.held[k] = 0.0;
                if !can_kick {
                    match act {
                        Action::Pass => pad.tackle = true,
                        Action::Shoot => pad.slide = true,
                        _ => {}
                    }
                }
            }
            if now.is_down(b) {
                self.held[k] += dt;
            }
            if now.just_released(prev, b) {
                if self.armed[k] && can_kick {
                    let c = Fx::from_f32((self.held[k] / FULL_CHARGE).min(1.0));
                    match k {
                        0 => pad.pass = Some(c),
                        1 => pad.shoot = Some(c),
                        _ => pad.through = Some(c),
                    }
                }
                self.armed[k] = false;
                self.held[k] = 0.0;
            }
        }
        pad
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn st(buttons: &[Button], stick: (f32, f32)) -> InputState {
        let mut s = InputState::default();
        for b in buttons {
            s.set(*b, true);
        }
        s.stick = trino_core::Vec2::new(stick.0, stick.1);
        s
    }

    #[test]
    fn stick_up_runs_away_from_the_camera() {
        let mut c = Controls::default();
        let pad = c.pad(
            &st(&[], (0.0, 1.0)),
            &InputState::default(),
            1.0 / 60.0,
            false,
        );
        assert!(pad.mv.y < Fx::ZERO && pad.mv.x == Fx::ZERO);
        let pad = c.pad(
            &st(&[Button::DpadRight], (0.0, 0.0)),
            &InputState::default(),
            1.0 / 60.0,
            false,
        );
        assert_eq!(pad.mv.x, -Fx::ONE);
    }

    #[test]
    fn a_held_shot_fires_on_release_with_its_charge() {
        let mut c = Controls::default();
        let none = InputState::default();
        let down = st(&[Button::B], (0.0, 0.0));
        let mut prev = none;
        let mut fired = None;
        for f in 0..40 {
            let now = if f < 30 { down } else { none };
            let pad = c.pad(&now, &prev, 1.0 / 60.0, true);
            assert!(!pad.slide, "with the ball, B is a shot, not a slide");
            if pad.shoot.is_some() {
                fired = pad.shoot;
            }
            prev = now;
        }
        let ch = fired.expect("released").to_f32();
        assert!((ch - 0.5 / FULL_CHARGE).abs() < 0.05, "{ch}");
    }

    #[test]
    fn a_press_without_the_ball_tackles_and_never_kicks_later() {
        let mut c = Controls::default();
        let none = InputState::default();
        let down = st(&[Button::B], (0.0, 0.0));
        let pad = c.pad(&down, &none, 1.0 / 60.0, false);
        assert!(pad.slide);
        // He wins the ball while B is still held: the release is not a shot.
        let pad = c.pad(&down, &down, 1.0 / 60.0, true);
        assert!(pad.shoot.is_none());
        let pad = c.pad(&none, &down, 1.0 / 60.0, true);
        assert!(pad.shoot.is_none());
    }

    #[test]
    fn l_switches_on_press_only() {
        let mut c = Controls::default();
        let l = st(&[Button::L], (0.0, 0.0));
        assert!(c.pad(&l, &InputState::default(), 0.016, false).switch);
        assert!(!c.pad(&l, &l, 0.016, false).switch);
    }

    #[test]
    fn rebinding_swaps_so_no_button_does_two_things() {
        let mut b = Bindings::DEFAULT;
        // Shoot onto A: pass takes B.
        assert!(b.assign(Action::Shoot, Button::A));
        assert_eq!(b.of(Action::Shoot), Button::A);
        assert_eq!(b.of(Action::Pass), Button::B);
        // A free button just moves.
        assert!(b.assign(Action::Sprint, Button::R2));
        assert_eq!(b.of(Action::Sprint), Button::R2);
        assert!(!b.0.contains(&Button::R));
        // The D-pad moves the player: never an action.
        assert!(!b.assign(Action::Pass, Button::DpadUp));
        for i in 0..ACTIONS {
            assert!(!b.0[i + 1..].contains(&b.0[i]), "{b:?}");
        }
        // Saved and restored.
        assert_eq!(Bindings::from_bytes(&b.to_bytes()), b);
    }

    #[test]
    fn broken_saved_bindings_fall_back_to_the_defaults() {
        assert_eq!(Bindings::from_bytes(&[0; ACTIONS]), Bindings::DEFAULT);
        assert_eq!(Bindings::from_bytes(&[200; ACTIONS]), Bindings::DEFAULT);
        // The D-pad's indices.
        assert_eq!(
            Bindings::from_bytes(&[8, 9, 10, 11, 4, 5, 6]),
            Bindings::DEFAULT
        );
    }

    #[test]
    fn a_pad_control_that_presses_two_buttons_fires_one_action() {
        let mut c = Controls::default();
        c.bindings.assign(Action::Switch, Button::L);
        c.bindings.assign(Action::Keeper, Button::L2);
        c.bindings.assign(Action::Sprint, Button::Select);
        let none = InputState::default();
        // LT on a PC pad: L and L2 both down.
        let mut lt = st(&[Button::L, Button::L2], (0.0, 0.0));
        lt.pad = true;
        let pad = c.pad(&lt, &none, 0.016, false);
        assert!(pad.keeper && !pad.switch, "LT is L2 only");
        // LB alone is L.
        let mut lb = st(&[Button::L], (0.0, 0.0));
        lb.pad = true;
        assert!(c.pad(&lb, &none, 0.016, false).switch);
        // The stick click (Select + L3) doesn't sprint through Select.
        let mut ls = st(&[Button::Select, Button::L3], (0.0, 0.0));
        ls.pad = true;
        assert!(!c.pad(&ls, &none, 0.016, false).sprint);
        // Not a pad (a 3DS with ZL and L both held): both count.
        let both = st(&[Button::L, Button::L2], (0.0, 0.0));
        let pad = c.pad(&both, &none, 0.016, false);
        assert!(pad.keeper && pad.switch);
    }

    #[test]
    fn the_match_reads_the_rebound_buttons() {
        let mut c = Controls::default();
        c.bindings.assign(Action::Pass, Button::Y);
        c.bindings.assign(Action::Switch, Button::L2);
        c.bindings.assign(Action::Sprint, Button::L3);
        let none = InputState::default();
        // Y now passes (and A, which keeper took over, takes the keeper).
        let y = st(&[Button::Y], (0.0, 0.0));
        c.pad(&y, &none, 0.1, true);
        assert!(c.pad(&none, &y, 0.016, true).pass.is_some());
        let a = st(&[Button::A], (0.0, 0.0));
        let pad = c.pad(&a, &none, 0.016, true);
        assert!(pad.keeper && pad.pass.is_none());
        assert!(
            c.pad(&st(&[Button::L2], (0.0, 0.0)), &none, 0.016, false)
                .switch
        );
        assert!(
            !c.pad(&st(&[Button::L], (0.0, 0.0)), &none, 0.016, false)
                .switch
        );
        assert!(
            c.pad(&st(&[Button::L3], (0.0, 0.0)), &none, 0.016, false)
                .sprint
        );
        assert!(
            !c.pad(&st(&[Button::R], (0.0, 0.0)), &none, 0.016, false)
                .sprint
        );
    }
}
