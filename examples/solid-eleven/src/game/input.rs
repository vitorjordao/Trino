//! Buttons to a [`Pad`]: the stick in pitch directions (the broadcast
//! camera looks north, so stick-up runs to -z), kicks charged while held and
//! played on release, tackles on press.
//!
//! | Button (PC key) | With the ball | Without it |
//! |---|---|---|
//! | A (Z) | pass (hold = harder) | standing tackle |
//! | B (X) | shoot (hold = power) | sliding tackle |
//! | X (C) | through ball / cross (hold = lofted) | — |
//! | L (Q) | — | switch player |
//! | R (E) | sprint | sprint |

use crate::sim::engine::Pad;
use crate::sim::fx::{Fx, V2};
use trino_core::{Button, InputState};

/// Seconds of holding for a full-power kick.
pub const FULL_CHARGE: f32 = 0.9;

/// Kick buttons, in `Controls::held` order.
const KICKS: [Button; 3] = [Button::A, Button::B, Button::X];

#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Controls {
    /// Seconds each kick button has been held (with the ball).
    pub held: [f32; 3],
    /// The press started while the side could kick (else a release is not
    /// a kick: a slide pressed without the ball must not shoot later).
    armed: [bool; 3],
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
            pad.mv = V2::new(Fx::from_f32(stick.x * k), Fx::from_f32(-stick.y * k));
        }
        pad.sprint = now.is_down(Button::R) || now.triggers.y > 0.5;
        pad.switch = now.just_pressed(prev, Button::L);
        for (k, b) in KICKS.into_iter().enumerate() {
            if now.just_pressed(prev, b) {
                self.armed[k] = can_kick;
                self.held[k] = 0.0;
                if !can_kick {
                    match b {
                        Button::A => pad.tackle = true,
                        Button::B => pad.slide = true,
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
        assert_eq!(pad.mv.x, Fx::ONE);
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
}
