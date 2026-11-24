//! Controller rumble (Bongo-Kart #180, ADR 0045 §2): what each player's
//! motors were asked to play, scaled by the player's setting, and until when.
//!
//! [`HostedRumble`] is the bookkeeping every backend with motors shares (the
//! PC, the Web, the N64's Rumble Pak): a game's
//! [`Platform::rumble`](crate::Platform::rumble) lands here, scaled by
//! [`AccessSettings::rumble`](crate::AccessSettings::rumble) (0 % plays
//! nothing), [`Platform::rumble_state`](crate::Platform::rumble_state) reads
//! it back by the platform's clock, and the backend forwards each new request
//! to the motors it has ([`HostedRumble::take_sent`]). A backend without
//! motors (the 3DS) keeps the trait's defaults: nothing plays, nothing is
//! reported.
//!
//! No float `Option` anywhere (the N64 pitfall): an idle motor is
//! [`Rumble::OFF`].

use crate::players::MAX_PLAYERS;

/// Two motors' strengths, 0..=1: `low` the heavy (low-frequency) one, `high`
/// the light one. Pads with one motor take the stronger of the two.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub struct Rumble {
    pub low: f32,
    pub high: f32,
}

impl Rumble {
    pub const OFF: Rumble = Rumble {
        low: 0.0,
        high: 0.0,
    };

    /// Both strengths in 0..=1 (NaN → 0).
    pub fn clamped(self) -> Rumble {
        let c = |v: f32| if v.is_nan() { 0.0 } else { v.clamp(0.0, 1.0) };
        Rumble {
            low: c(self.low),
            high: c(self.high),
        }
    }

    /// The same, times `scale` (a player's setting, 0..=1).
    pub fn scaled(self, scale: f32) -> Rumble {
        let s = if scale.is_nan() {
            0.0
        } else {
            scale.clamp(0.0, 1.0)
        };
        let c = self.clamped();
        Rumble {
            low: c.low * s,
            high: c.high * s,
        }
    }

    /// Both motors still.
    pub fn is_off(&self) -> bool {
        self.low <= 0.0 && self.high <= 0.0
    }

    /// The stronger motor (a single-motor pad, the N64's Rumble Pak).
    pub fn strongest(&self) -> f32 {
        if self.low > self.high {
            self.low
        } else {
            self.high
        }
    }
}

/// An on/off motor (the N64's Rumble Pak) run at a strength: on for that
/// share of frames, spread evenly by an accumulator, so 0.5 is every other
/// frame and 0.25 one frame in four. A request starts with the motor on, so
/// a short bump is felt at once.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct DutyCycle {
    acc: f32,
}

impl DutyCycle {
    /// Full on the next frame a strength arrives.
    const START: f32 = 0.999;

    pub const fn new() -> DutyCycle {
        DutyCycle { acc: Self::START }
    }

    /// One frame at `strength` (0..=1, NaN and below 0 are off): is the
    /// motor on this frame?
    pub fn step(&mut self, strength: f32) -> bool {
        if strength.is_nan() || strength <= 0.0 {
            self.acc = Self::START;
            return false;
        }
        self.acc += if strength > 1.0 { 1.0 } else { strength };
        if self.acc >= 1.0 {
            self.acc -= 1.0;
            true
        } else {
            false
        }
    }
}

impl Default for DutyCycle {
    fn default() -> DutyCycle {
        DutyCycle::new()
    }
}

/// Whether a player's rumble can be felt
/// ([`Platform::rumble_support`](crate::Platform::rumble_support)): a game
/// shows its vibration option where it does something and hides it where it
/// can't (no placebo setting).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum RumbleSupport {
    /// This platform has no motors (the 3DS, a PC build without the pad
    /// layer, a browser without haptics).
    #[default]
    Unsupported,
    /// Motors are possible but the player's controller has none right now
    /// (no pad, a pad without motors, no Rumble Pak in that port).
    Disconnected,
    /// The player's controller rumbles.
    Ready,
}

#[derive(Clone, Copy, PartialEq, Debug, Default)]
struct Slot {
    rumble: Rumble,
    /// When it stops, on the platform's microsecond clock.
    until_us: u64,
    /// How long it was asked to last (what the motors are sent).
    ms: u32,
    /// A request the backend hasn't forwarded yet.
    pending: bool,
}

/// Every player's requested rumble (see the module's docs).
#[derive(Clone, Copy, Debug, Default)]
pub struct HostedRumble {
    slots: [Slot; MAX_PLAYERS],
}

impl HostedRumble {
    pub const fn new() -> HostedRumble {
        const IDLE: Slot = Slot {
            rumble: Rumble::OFF,
            until_us: 0,
            ms: 0,
            pending: false,
        };
        HostedRumble {
            slots: [IDLE; MAX_PLAYERS],
        }
    }

    /// Player `player`'s motors play `low`/`high` for `ms` from `now_us`,
    /// times `scale` (the player's setting): a new request replaces the
    /// last; 0 ms, both strengths 0 or a scale of 0 stop them.
    pub fn request(&mut self, player: u8, low: f32, high: f32, ms: u32, now_us: u64, scale: f32) {
        let Some(s) = self.slots.get_mut(player as usize) else {
            return;
        };
        let r = Rumble { low, high }.scaled(scale);
        let (r, ms) = if ms == 0 || r.is_off() {
            (Rumble::OFF, 0)
        } else {
            (r, ms)
        };
        *s = Slot {
            rumble: r,
            until_us: now_us.saturating_add(ms as u64 * 1000),
            ms,
            pending: true,
        };
    }

    /// What player `player`'s motors play at `now_us` ([`Rumble::OFF`] once
    /// the request ran out).
    pub fn state(&self, player: u8, now_us: u64) -> Rumble {
        match self.slots.get(player as usize) {
            Some(s) if now_us < s.until_us => s.rumble,
            _ => Rumble::OFF,
        }
    }

    /// The next request the backend hasn't sent to the motors yet: the
    /// player, the strengths (already scaled) and how long; `OFF` and 0 ms
    /// stop them.
    pub fn take_sent(&mut self) -> Option<(u8, Rumble, u32)> {
        let (i, s) = self.slots.iter_mut().enumerate().find(|(_, s)| s.pending)?;
        s.pending = false;
        Some((i as u8, s.rumble, s.ms))
    }

    /// Stop every motor (a pause menu, the game losing focus).
    pub fn stop_all(&mut self, now_us: u64) {
        for p in 0..MAX_PLAYERS as u8 {
            if !self.state(p, now_us).is_off() {
                self.request(p, 0.0, 0.0, 0, now_us, 1.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_request_is_scaled_lasts_its_time_and_is_sent_once() {
        let mut h = HostedRumble::new();
        h.request(1, 1.0, 0.5, 200, 1_000, 0.5);
        assert_eq!(
            h.state(1, 1_000),
            Rumble {
                low: 0.5,
                high: 0.25
            }
        );
        assert_eq!(h.state(0, 1_000), Rumble::OFF);
        assert_eq!(
            h.state(1, 200_999),
            Rumble {
                low: 0.5,
                high: 0.25
            }
        );
        assert_eq!(h.state(1, 201_000), Rumble::OFF);
        assert_eq!(
            h.take_sent(),
            Some((
                1,
                Rumble {
                    low: 0.5,
                    high: 0.25
                },
                200
            ))
        );
        assert_eq!(h.take_sent(), None);
    }

    #[test]
    fn zero_scale_nan_and_zero_time_play_nothing() {
        let mut h = HostedRumble::new();
        h.request(0, 1.0, 1.0, 100, 0, 0.0);
        assert!(h.state(0, 0).is_off());
        h.request(0, f32::NAN, 2.0, 100, 0, 1.0);
        assert_eq!(
            h.state(0, 0),
            Rumble {
                low: 0.0,
                high: 1.0
            }
        );
        h.request(0, 1.0, 1.0, 0, 0, 1.0);
        assert!(h.state(0, 0).is_off());
        h.request(9, 1.0, 1.0, 100, 0, 1.0); // no such player
        assert_eq!(h.take_sent(), Some((0, Rumble::OFF, 0)));
    }

    #[test]
    fn a_duty_cycle_runs_its_share_of_frames_and_starts_on() {
        for (strength, on) in [(1.0, 60), (0.5, 30), (0.25, 15), (0.1, 6), (2.0, 60)] {
            let mut d = DutyCycle::new();
            let frames: Vec<bool> = (0..60).map(|_| d.step(strength)).collect();
            assert!(frames[0], "{strength}: a request starts on");
            assert_eq!(frames.iter().filter(|f| **f).count(), on, "{strength}");
        }
        let mut d = DutyCycle::new();
        assert!(d.step(0.5) && !d.step(0.5) && d.step(0.5));
        // Off resets: the next request starts on again.
        assert!(!d.step(0.0) && !d.step(f32::NAN) && d.step(0.25) && !d.step(0.25));
    }

    #[test]
    fn stop_all_sends_a_stop_to_the_playing_motors_only() {
        let mut h = HostedRumble::new();
        h.request(2, 0.3, 0.0, 1000, 0, 1.0);
        let _ = h.take_sent();
        h.stop_all(10);
        assert!(h.state(2, 10).is_off());
        assert_eq!(h.take_sent(), Some((2, Rumble::OFF, 0)));
        assert_eq!(h.take_sent(), None);
        assert_eq!(
            Rumble {
                low: 0.2,
                high: 0.7
            }
            .strongest(),
            0.7
        );
    }
}
