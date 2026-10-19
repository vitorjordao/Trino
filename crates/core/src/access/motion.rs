//! Motion, flashes and held inputs, each honoring [`AccessSettings`]:
//! [`CameraFx`] (shake, punch, FOV kick), [`ScreenFlash`] (a full-screen
//! flash the guard keeps to [`MAX_FLASHES_PER_SECOND`]), [`FlashMonitor`]
//! (finds strobes in a stream of frames: the PC's strict mode runs it over
//! full-screen fills) and [`HoldToggle`].

use super::AccessSettings;
use crate::input::{Button, InputState};
use crate::math::{Color, Rect, Vec3};
use crate::math3d;
use crate::render::{Renderer, Screen};
use crate::render3d::Camera3;

/// What `reduce_motion` leaves of a shake or a punch.
pub const REDUCED_MOTION: f32 = 0.2;

/// Camera effects a game triggers and applies to its camera each frame.
/// Under `reduce_motion` shakes and punches keep [`REDUCED_MOTION`] of
/// their size and FOV kicks are dropped. Deterministic (the core's sin).
#[derive(Clone, Copy, Debug, Default)]
pub struct CameraFx {
    shake: f32,
    shake_left: f32,
    shake_len: f32,
    punch: Vec3,
    punch_left: f32,
    punch_len: f32,
    fov: f32,
    fov_left: f32,
    fov_len: f32,
    time: f32,
}

impl CameraFx {
    pub const fn new() -> CameraFx {
        CameraFx {
            shake: 0.0,
            shake_left: 0.0,
            shake_len: 0.0,
            punch: Vec3::ZERO,
            punch_left: 0.0,
            punch_len: 0.0,
            fov: 0.0,
            fov_left: 0.0,
            fov_len: 0.0,
            time: 0.0,
        }
    }

    /// Shake by up to `amplitude` world units, fading out over `seconds`.
    /// A stronger shake replaces a weaker one in progress.
    pub fn shake(&mut self, amplitude: f32, seconds: f32) {
        if self.shake * self.shake_fade() <= amplitude.abs() {
            self.shake = amplitude.abs();
            self.shake_left = seconds.max(0.0);
            self.shake_len = seconds.max(1e-3);
        }
    }

    /// Push the camera by `offset`, springing back over `seconds`.
    pub fn punch(&mut self, offset: Vec3, seconds: f32) {
        self.punch = offset;
        self.punch_left = seconds.max(0.0);
        self.punch_len = seconds.max(1e-3);
    }

    /// Widen (or narrow, negative) the field of view by `radians`, easing
    /// back over `seconds`.
    pub fn kick_fov(&mut self, radians: f32, seconds: f32) {
        self.fov = radians;
        self.fov_left = seconds.max(0.0);
        self.fov_len = seconds.max(1e-3);
    }

    pub fn update(&mut self, dt: f32) {
        self.time += dt;
        self.shake_left = (self.shake_left - dt).max(0.0);
        self.punch_left = (self.punch_left - dt).max(0.0);
        self.fov_left = (self.fov_left - dt).max(0.0);
    }

    /// Whether anything is still moving the camera.
    pub fn active(&self) -> bool {
        self.shake_left > 0.0 || self.punch_left > 0.0 || self.fov_left > 0.0
    }

    fn shake_fade(&self) -> f32 {
        if self.shake_left > 0.0 {
            self.shake_left / self.shake_len
        } else {
            0.0
        }
    }

    /// The eye's offset this frame.
    pub fn offset(&self, settings: &AccessSettings) -> Vec3 {
        let k = if settings.reduce_motion {
            REDUCED_MOTION
        } else {
            1.0
        };
        let a = self.shake * self.shake_fade() * k;
        let t = self.time;
        // Three incommensurate sines per axis: noisy enough, repeatable.
        let shake = Vec3::new(
            a * (0.6 * math3d::sin(t * 47.0) + 0.4 * math3d::sin(t * 83.0 + 1.3)),
            a * (0.6 * math3d::sin(t * 53.0 + 2.1) + 0.4 * math3d::sin(t * 97.0)),
            a * 0.5 * math3d::sin(t * 61.0 + 0.7),
        );
        let p = if self.punch_left > 0.0 {
            let f = self.punch_left / self.punch_len;
            self.punch * (f * f * k)
        } else {
            Vec3::ZERO
        };
        shake + p
    }

    /// The field of view's change this frame (0 under `reduce_motion`).
    pub fn fov_delta(&self, settings: &AccessSettings) -> f32 {
        if settings.reduce_motion || self.fov_left <= 0.0 {
            return 0.0;
        }
        let f = self.fov_left / self.fov_len;
        self.fov * f * f
    }

    /// Move `camera`'s eye and target by [`offset`](Self::offset) and add
    /// [`fov_delta`](Self::fov_delta).
    pub fn apply(&self, camera: &mut Camera3, settings: &AccessSettings) {
        let o = self.offset(settings);
        camera.eye += o;
        camera.target += o;
        camera.fov_y += self.fov_delta(settings);
    }
}

/// The most full-screen flashes a second under `flash_guard` (WCAG 2.3.1's
/// "three flashes").
pub const MAX_FLASHES_PER_SECOND: usize = 3;
/// The strongest a guarded flash gets (alpha of its color, 0..=255).
pub const GUARDED_ALPHA: u8 = 160;

/// A full-screen flash (a hit, lightning, a camera flash) drawn with one
/// `fill_rect` that fades out. Under `flash_guard` a flash that would be
/// the fourth within a second is dropped and every flash keeps its alpha
/// at or under [`GUARDED_ALPHA`].
#[derive(Clone, Copy, Debug)]
pub struct ScreenFlash {
    color: Color,
    left: f32,
    len: f32,
    now: f32,
    /// When the last flashes started (the guard's window).
    recent: [f32; MAX_FLASHES_PER_SECOND],
    /// Flashes the guard dropped since [`new`](Self::new).
    pub dropped: u32,
}

impl ScreenFlash {
    pub const fn new() -> ScreenFlash {
        ScreenFlash {
            color: Color::TRANSPARENT,
            left: 0.0,
            len: 1.0,
            now: 0.0,
            recent: [f32::NEG_INFINITY; MAX_FLASHES_PER_SECOND],
            dropped: 0,
        }
    }

    /// Start a flash of `color` (its alpha is the peak) fading over
    /// `seconds`. `false` when the guard dropped it.
    pub fn trigger(&mut self, color: Color, seconds: f32, settings: &AccessSettings) -> bool {
        if settings.flash_guard {
            // The oldest of the last three must be a second old.
            let oldest = self.recent.iter().copied().fold(f32::INFINITY, f32::min);
            if self.now - oldest < 1.0 {
                self.dropped += 1;
                return false;
            }
        }
        let slot = self
            .recent
            .iter()
            .enumerate()
            .fold(
                (0, f32::INFINITY),
                |(i, t), (j, &u)| {
                    if u < t { (j, u) } else { (i, t) }
                },
            )
            .0;
        self.recent[slot] = self.now;
        self.color = if settings.flash_guard {
            Color::rgba(color.r, color.g, color.b, color.a.min(GUARDED_ALPHA))
        } else {
            color
        };
        self.left = seconds.max(0.0);
        self.len = seconds.max(1e-3);
        true
    }

    pub fn update(&mut self, dt: f32) {
        self.now += dt;
        self.left = (self.left - dt).max(0.0);
    }

    /// The color the flash covers the screen with this frame (alpha 0 when
    /// none).
    pub fn current(&self) -> Color {
        if self.left <= 0.0 {
            return Color::TRANSPARENT;
        }
        let a = self.color.a as f32 * (self.left / self.len);
        Color::rgba(self.color.r, self.color.g, self.color.b, a as u8)
    }

    /// Cover the current screen with [`current`](Self::current).
    pub fn draw(&self, renderer: &mut dyn Renderer) {
        let c = self.current();
        if c.a == 0 {
            return;
        }
        let which = renderer.screen();
        if let Some(size) = renderer.screen_size(which).or_else(|| {
            (which == Screen::Top).then(|| {
                let caps = renderer.caps();
                crate::math::Vec2::new(caps.screen_width as f32, caps.screen_height as f32)
            })
        }) {
            renderer.fill_rect(Rect::new(0.0, 0.0, size.x, size.y), c);
        }
    }
}

impl Default for ScreenFlash {
    fn default() -> Self {
        Self::new()
    }
}

/// How much brighter a frame must get to count as a flash (relative
/// luminance × coverage).
pub const FLASH_STEP: f32 = 0.1;

/// Finds strobes in a stream of frames: each frame reports the brightness a
/// full-screen overlay adds (relative luminance × alpha × coverage, 0 when
/// there is none); a rise of [`FLASH_STEP`] or more starts a flash. More
/// than [`MAX_FLASHES_PER_SECOND`] in a second is a strobe. The PC
/// renderer runs it over its full-screen `fill_rect`s in strict mode and
/// logs a warning; a test can feed it anything.
#[derive(Clone, Copy, Debug)]
pub struct FlashMonitor {
    starts: [f32; 8],
    next: usize,
    level: f32,
    /// Strobes found (each counted once, when it starts).
    pub strobes: u32,
    strobing: bool,
}

impl FlashMonitor {
    pub const fn new() -> FlashMonitor {
        FlashMonitor {
            starts: [f32::NEG_INFINITY; 8],
            next: 0,
            level: 0.0,
            strobes: 0,
            strobing: false,
        }
    }

    /// The brightness an overlay adds: relative luminance × alpha ×
    /// the part of the screen it covers.
    pub fn level_of(color: Color, coverage: f32) -> f32 {
        super::color::luminance(color) * (color.a as f32 / 255.0) * coverage.clamp(0.0, 1.0)
    }

    /// Report a frame at `now` seconds. `true` on the frame a strobe starts
    /// (the fourth flash within a second).
    pub fn frame(&mut self, now: f32, level: f32) -> bool {
        let rise = level - self.level >= FLASH_STEP;
        self.level = level;
        if rise {
            self.starts[self.next] = now;
            self.next = (self.next + 1) % self.starts.len();
        }
        let strobing = self.flashes_within(now) > MAX_FLASHES_PER_SECOND;
        let started = strobing && !self.strobing;
        self.strobing = strobing;
        if started {
            self.strobes += 1;
        }
        started
    }

    /// Flashes that started in the second up to `now`.
    pub fn flashes_within(&self, now: f32) -> usize {
        self.starts
            .iter()
            .filter(|&&t| now - t < 1.0 && t <= now)
            .count()
    }
}

impl Default for FlashMonitor {
    fn default() -> Self {
        Self::new()
    }
}

/// A held action (sprint, aim, crouch) as the player wants it: held down,
/// or — with `hold_to_toggle` — one press on, the next press off.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct HoldToggle {
    on: bool,
}

impl HoldToggle {
    pub const fn new() -> HoldToggle {
        HoldToggle { on: false }
    }

    /// The action's state this frame.
    pub fn update(
        &mut self,
        input: &InputState,
        prev: &InputState,
        button: Button,
        settings: &AccessSettings,
    ) -> bool {
        self.on = if settings.hold_to_toggle {
            self.on ^ input.just_pressed(prev, button)
        } else {
            input.is_down(button)
        };
        self.on
    }

    pub fn is_on(&self) -> bool {
        self.on
    }

    /// Off (a new life, a menu opened).
    pub fn reset(&mut self) {
        self.on = false;
    }
}
