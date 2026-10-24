//! Stereoscopic 3D: the 3DS top screen's two eyes (issue #63, ADR 0032).
//!
//! The game draws its frame **once**. A target with a stereo screen
//! ([`crate::Caps::stereo`]: the 3DS top screen, and the PC's 3DS profile as
//! a preview) records the top screen's draws ([`StereoCmd`]) and replays
//! them per eye, each eye's 3D through an **off-axis** projection: the eye
//! moves sideways by half the separation and the picture's center moves
//! back by as much at the convergence depth, so a point there lands on the
//! same pixel in both eyes (zero parallax), nearer points pop out of the
//! screen and farther ones sink into it. Screen y never changes between the
//! eyes (no toe-in: no vertical parallax).
//!
//! - [`StereoSettings`] ([`crate::Renderer::set_stereo`], cleared at
//!   `begin_frame`, like shadows): on/off, the eye separation at the full
//!   3D slider, the convergence depth, and the [`StereoPolicy`].
//! - The separation in use is the 3D slider's position times
//!   `max_separation`. The slider at 0 (or an old 2DS, which has no stereo
//!   screen) is the mono frame: nothing is recorded and nothing costs more.
//! - [`LayerDepth`] ([`crate::Renderer::set_layer_depth`]) floats the 2D
//!   drawn after it (HUD, UI): pixels of parallax at the full slider,
//!   positive out of the screen.
//! - Every other target (N64, PC native, Web) is mono: the defaults are
//!   no-ops, and game code is the same everywhere.
//!
//! The per-eye cost: a GPU T&L draw (the 3DS's PICA200 path) runs its
//! vertex program twice, a CPU T&L draw is transformed twice, and the 2D
//! is drawn twice. [`StereoPolicy::Auto`] drops to mono for the frames that
//! miss the budget ([`StereoGovernor`]); [`StereoStats`] says which eye
//! count and why in `TRINO_FRAME`.

use crate::light::LocalLight;
use crate::math::{Color, Rect, Vec2, Vec3};
use crate::math3d::Mat34;
use crate::render::{
    BillboardParams, FontId, Material, ModelId, ModelParams, Renderer, SpriteId, SpriteParams,
    TexFilter, TextParams,
};
use crate::render3d::{Camera3, Light};
use crate::shadow::{ShadowPlane, ShadowSettings};
use crate::water::Ripple;

/// One frame's stereo request ([`crate::Renderer::set_stereo`]).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StereoSettings {
    /// Off draws one picture, whatever the slider says.
    pub enabled: bool,
    /// Distance between the two eyes at the full 3D slider, world units.
    /// The slider scales it down to 0.
    pub max_separation: f32,
    /// View depth with zero parallax, world units: the screen's plane.
    /// Nearer points pop out, farther ones sink in. Put it on what the
    /// player looks at (the character, the ball).
    pub convergence: f32,
    /// What to do when two eyes miss the frame budget.
    pub policy: StereoPolicy,
    /// The frame time [`StereoPolicy::Auto`] holds stereo frames to,
    /// microseconds (16 667 = 60 fps; a 30 fps game says 33 333).
    pub budget_us: u32,
    /// A fixed slider position (0..=1) in place of the hardware's: tests,
    /// captures and the editor's preview. Games leave it `None`: the slider
    /// is the player's.
    pub slider: Option<f32>,
}

impl StereoSettings {
    /// Off.
    pub const OFF: StereoSettings = StereoSettings {
        enabled: false,
        max_separation: 0.0,
        convergence: 4.0,
        policy: StereoPolicy::Auto,
        budget_us: 16_667,
        slider: None,
    };

    /// On, `max_separation` apart at the full slider, converging at
    /// `convergence`; the [`StereoPolicy::Auto`] policy at 60 fps.
    pub const fn new(max_separation: f32, convergence: f32) -> StereoSettings {
        StereoSettings {
            enabled: true,
            max_separation,
            convergence,
            ..Self::OFF
        }
    }

    /// These settings with the slider fixed at `slider` (0..=1).
    pub const fn with_slider(mut self, slider: f32) -> StereoSettings {
        self.slider = Some(slider);
        self
    }
}

impl Default for StereoSettings {
    fn default() -> Self {
        Self::OFF
    }
}

/// What a stereo frame does when two eyes cost too much.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum StereoPolicy {
    /// Drop to one eye for the frames past
    /// [`StereoSettings::budget_us`] ([`StereoGovernor`]), and come back
    /// once there is room again.
    #[default]
    Auto,
    /// Always two eyes while the slider is up, whatever they cost.
    Always,
}

/// Which picture a target is drawing ([`crate::Renderer::stereo_eye`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Eye {
    /// The one picture of a mono frame.
    #[default]
    Mono,
    Left,
    Right,
}

impl Eye {
    /// -1 left, +1 right, 0 mono: the side the eye moves to.
    pub const fn sign(self) -> f32 {
        match self {
            Eye::Mono => 0.0,
            Eye::Left => -1.0,
            Eye::Right => 1.0,
        }
    }
}

/// The screen-plane offset of the 2D drawn after
/// [`crate::Renderer::set_layer_depth`] (HUDs, UI, captions): pixels of
/// parallax between the eyes at the full slider. 0 is the screen's plane
/// (the default), positive pops out toward the player, negative sinks into
/// the screen. Each eye moves half of it, the slider scales it.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct LayerDepth(pub i8);

impl LayerDepth {
    /// The screen's plane.
    pub const SCREEN: LayerDepth = LayerDepth(0);

    /// How far `eye` moves this layer's 2D right, pixels, at `strength`
    /// (the slider, 0..=1). Popping out (positive) moves the left eye's
    /// copy right and the right eye's left (crossed parallax).
    pub fn shift_px(self, eye: Eye, strength: f32) -> f32 {
        -eye.sign() * self.0 as f32 * 0.5 * strength
    }
}

/// Why a frame was drawn with one picture ([`StereoStats::mono`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum StereoMono {
    /// Two pictures were drawn.
    #[default]
    Stereo,
    /// The game didn't ask (no [`crate::Renderer::set_stereo`], or
    /// `enabled: false`).
    Off,
    /// The target has no stereo screen ([`crate::Caps::stereo`]: N64, PC
    /// native, the Web, an old 2DS or a New 2DS XL).
    NoScreen,
    /// The 3D slider is down.
    Slider,
    /// [`StereoPolicy::Auto`] dropped this frame to one eye.
    Budget,
    /// The top screen already had draws when `set_stereo` came: call it
    /// right after `begin_frame`.
    Late,
}

impl StereoMono {
    /// The `TRINO_FRAME stereo=` word.
    pub const fn label(self) -> &'static str {
        match self {
            StereoMono::Stereo => "on",
            StereoMono::Off => "off",
            StereoMono::NoScreen => "none",
            StereoMono::Slider => "slider0",
            StereoMono::Budget => "budget",
            StereoMono::Late => "late",
        }
    }
}

/// What the last frame did ([`crate::FrameStats::stereo`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct StereoStats {
    /// Pictures drawn on the top screen: 1 or 2.
    pub eyes: u8,
    /// The slider position used, percent.
    pub slider_pct: u8,
    /// Why one picture, or [`StereoMono::Stereo`].
    pub mono: StereoMono,
    /// Top-screen calls recorded and replayed per eye.
    pub commands: u16,
}

impl StereoStats {
    /// The `TRINO_FRAME` field: `stereo=on@100%` or `stereo=slider0`.
    pub fn write(&self, out: &mut impl core::fmt::Write) -> core::fmt::Result {
        if self.eyes == 2 {
            write!(out, "stereo=on@{}%", self.slider_pct)
        } else {
            write!(out, "stereo={}", self.mono.label())
        }
    }
}

/// The resolved stereo of one frame: what a backend draws with.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct StereoFrame {
    /// The eyes' distance this frame, world units (0 = mono).
    pub separation: f32,
    /// The zero-parallax depth.
    pub convergence: f32,
    /// The slider position used, 0..=1 (scales [`LayerDepth`]).
    pub strength: f32,
    /// Why it is mono, or [`StereoMono::Stereo`].
    pub mono: StereoMono,
}

impl StereoFrame {
    /// One picture, for `why`.
    pub const fn mono(why: StereoMono) -> StereoFrame {
        StereoFrame {
            separation: 0.0,
            convergence: 1.0,
            strength: 0.0,
            mono: why,
        }
    }

    /// The frame `settings` asks for on a target with (`screen`) or without
    /// a stereo screen, its slider at `hw_slider` (0..=1), and `budget_ok`
    /// from the [`StereoGovernor`] (true when the policy is
    /// [`StereoPolicy::Always`]).
    pub fn resolve(
        settings: Option<&StereoSettings>,
        screen: bool,
        hw_slider: f32,
        budget_ok: bool,
    ) -> StereoFrame {
        let Some(s) = settings.filter(|s| s.enabled) else {
            return Self::mono(StereoMono::Off);
        };
        if !screen {
            return Self::mono(StereoMono::NoScreen);
        }
        let slider = s.slider.unwrap_or(hw_slider);
        let slider = if slider.is_nan() {
            0.0
        } else {
            slider.clamp(0.0, 1.0)
        };
        if slider <= 0.0 || s.max_separation <= 0.0 || s.convergence <= 0.0 {
            return Self::mono(StereoMono::Slider);
        }
        if s.policy == StereoPolicy::Auto && !budget_ok {
            return Self::mono(StereoMono::Budget);
        }
        StereoFrame {
            separation: slider * s.max_separation,
            convergence: s.convergence,
            strength: slider,
            mono: StereoMono::Stereo,
        }
    }

    /// Two pictures.
    pub fn is_stereo(&self) -> bool {
        self.mono == StereoMono::Stereo
    }

    /// How far `eye` sits right of the mono camera, world units.
    pub fn eye_shift(&self, eye: Eye) -> f32 {
        eye.sign() * self.separation * 0.5
    }

    /// The stats of a frame drawn this way with `commands` recorded calls.
    pub fn stats(&self, commands: usize) -> StereoStats {
        StereoStats {
            eyes: if self.is_stereo() { 2 } else { 1 },
            slider_pct: (self.strength * 100.0 + 0.5) as u8,
            mono: self.mono,
            commands: commands.min(u16::MAX as usize) as u16,
        }
    }
}

/// Where a view-space point lands on screen for one eye (the T&L's
/// off-axis projection, [`crate::TnlContext::with_eye`]): `center` and
/// `focal` are the mono projection's, `shift` the eye's
/// ([`StereoFrame::eye_shift`]). Tests and self-test probes use it.
pub fn project_eye(v: Vec3, focal: f32, center: Vec2, shift: f32, convergence: f32) -> Vec2 {
    let cx = center.x + shift * focal / convergence;
    Vec2::new(
        cx + (v.x - shift) * focal / v.z,
        center.y - v.y * focal / v.z,
    )
}

/// The parallax of a point `z` deep between the right and the left eye,
/// pixels (right minus left): zero at the convergence depth, negative
/// (crossed: pops out) nearer, positive farther, approaching
/// `separation · focal / convergence` at infinity.
pub fn parallax_px(z: f32, separation: f32, focal: f32, convergence: f32) -> f32 {
    separation * focal * (1.0 / convergence - 1.0 / z)
}

/// [`StereoPolicy::Auto`]: drops to mono after `DROP` stereo frames in a
/// row over the budget, tries stereo again after `RETRY` mono frames.
/// Feed it every frame's work time (the frame less its vsync wait).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StereoGovernor {
    over: u8,
    mono_left: u16,
}

impl StereoGovernor {
    /// Stereo frames in a row over the budget before the drop.
    pub const DROP: u8 = 3;
    /// Mono frames before the next stereo try.
    pub const RETRY: u16 = 120;

    /// Whether this frame may draw two eyes.
    pub fn allows(&self) -> bool {
        self.mono_left == 0
    }

    /// The last frame took `work_us`; it drew two eyes (`stereo`) or one.
    pub fn frame(&mut self, stereo: bool, work_us: u32, budget_us: u32) {
        if self.mono_left > 0 {
            self.mono_left -= 1;
            return;
        }
        if !stereo {
            self.over = 0;
            return;
        }
        if work_us > budget_us {
            self.over = self.over.saturating_add(1);
            if self.over >= Self::DROP {
                self.over = 0;
                self.mono_left = Self::RETRY;
            }
        } else {
            self.over = 0;
        }
    }
}

/// A span of a [`StereoData`] arena: `start` and `len` items.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Span {
    pub start: u32,
    pub len: u32,
}

impl Span {
    fn of<'a, T>(self, items: &'a [T]) -> &'a [T] {
        let (s, e) = (self.start as usize, (self.start + self.len) as usize);
        items.get(s..e).unwrap_or(&[])
    }
}

/// One recorded top-screen call. Slices live in the [`StereoData`]
/// arenas; a backend keeps both (its own `Vec`s, reused every frame) and
/// [`replay`]s them once per eye.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum StereoCmd {
    Camera(Camera3),
    Light(Light),
    LocalLights(Span),
    Wetness(f32),
    Water {
        time: f32,
        ripples: Span,
    },
    Shadows(Option<ShadowSettings>),
    Receivers(Span),
    Filter(TexFilter),
    Depth(LayerDepth),
    Model {
        model: ModelId,
        matrix: Mat34,
        material: Material,
        params: ModelParams,
    },
    Billboard {
        sprite: SpriteId,
        pos: Vec3,
        params: BillboardParams,
    },
    Sprite {
        sprite: SpriteId,
        pos: Vec2,
        params: SpriteParams,
    },
    Rect {
        rect: Rect,
        color: Color,
    },
    Text {
        font: FontId,
        pos: Vec2,
        text: Span,
        params: TextParams,
    },
}

/// The arenas a recording's [`Span`]s point into.
#[derive(Clone, Copy, Default)]
pub struct StereoData<'a> {
    pub text: &'a [u8],
    pub lights: &'a [LocalLight],
    pub ripples: &'a [Ripple],
    pub planes: &'a [ShadowPlane],
}

/// Play `cmds` into `r`, in order. The backend has set its eye first and
/// stopped recording; its own `set_camera` applies the eye.
pub fn replay<R: Renderer + ?Sized>(r: &mut R, cmds: &[StereoCmd], data: &StereoData<'_>) {
    for c in cmds {
        match *c {
            StereoCmd::Camera(cam) => r.set_camera(&cam),
            StereoCmd::Light(l) => r.set_light(&l),
            StereoCmd::LocalLights(s) => r.set_local_lights(s.of(data.lights)),
            StereoCmd::Wetness(w) => r.set_wetness(w),
            StereoCmd::Water { time, ripples } => r.set_water(time, ripples.of(data.ripples)),
            StereoCmd::Shadows(s) => r.set_shadows(s.as_ref()),
            StereoCmd::Receivers(s) => r.set_shadow_receivers(s.of(data.planes)),
            StereoCmd::Filter(f) => r.set_texture_filter(f),
            StereoCmd::Depth(d) => r.set_layer_depth(d),
            StereoCmd::Model {
                model,
                matrix,
                material,
                params,
            } => r.draw_model_matrix(model, &matrix, material, &params),
            StereoCmd::Billboard {
                sprite,
                pos,
                params,
            } => r.draw_billboard(sprite, pos, &params),
            StereoCmd::Sprite {
                sprite,
                pos,
                params,
            } => r.draw_sprite(sprite, pos, &params),
            StereoCmd::Rect { rect, color } => r.fill_rect(rect, color),
            StereoCmd::Text {
                font,
                pos,
                text,
                params,
            } => {
                if let Ok(s) = core::str::from_utf8(text.of(data.text)) {
                    r.draw_text(font, pos, s, &params);
                }
            }
        }
    }
}

/// A 2D position moved for `eye` by the current layer depth.
pub fn shift_2d(pos: Vec2, depth: LayerDepth, eye: Eye, strength: f32) -> Vec2 {
    Vec2::new(pos.x + depth.shift_px(eye, strength), pos.y)
}

/// The camera an eye sees through: eye and target moved together along
/// the camera's right vector (parallel axes; the T&L adds the off-axis
/// shift of the picture's center, [`crate::TnlContext::with_eye`]).
pub fn eye_camera(camera: &Camera3, shift: f32) -> Camera3 {
    let v = camera.view();
    let right = Vec3::new(v.m[0][0], v.m[0][1], v.m[0][2]);
    Camera3 {
        eye: camera.eye + right * shift,
        target: camera.target + right * shift,
        fov_y: camera.fov_y,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::render3d::{DEFAULT_LIGHT, TnlContext};

    fn ctx() -> TnlContext {
        let cam = Camera3 {
            eye: Vec3::new(1.0, 2.0, -6.0),
            target: Vec3::new(0.5, 0.5, 0.0),
            fov_y: 1.0,
        };
        TnlContext::new(&cam, &DEFAULT_LIGHT, Vec2::new(400.0, 240.0))
    }

    /// Where world point `p` lands through `c` (the T&L's projection).
    fn screen(c: &TnlContext, p: Vec3) -> Vec2 {
        let v = c.view_point(p);
        Vec2::new(
            c.center().x + v.x * c.focal() / v.z,
            c.center().y - v.y * c.focal() / v.z,
        )
    }

    #[test]
    fn zero_parallax_at_the_convergence_depth() {
        let mono = ctx();
        let conv = 5.0;
        let (l, r) = (mono.with_eye(-0.1, conv), mono.with_eye(0.1, conv));
        // A point `conv` deep on the view axis, and one off it.
        for off in [0.0f32, 1.3, -0.7] {
            let v = Vec3::new(off, 0.4, conv);
            // World point with that view position: invert by search-free
            // construction (camera basis).
            let view = mono.camera().view();
            let (rx, ry, rz) = (
                Vec3::new(view.m[0][0], view.m[0][1], view.m[0][2]),
                Vec3::new(view.m[1][0], view.m[1][1], view.m[1][2]),
                Vec3::new(view.m[2][0], view.m[2][1], view.m[2][2]),
            );
            let p = mono.camera().eye + rx * v.x + ry * v.y + rz * v.z;
            let (a, b, m) = (screen(&l, p), screen(&r, p), screen(&mono, p));
            assert!((a.x - b.x).abs() < 1e-3, "{a:?} {b:?}");
            assert!((a.x - m.x).abs() < 1e-3, "{a:?} {m:?}");
        }
    }

    #[test]
    fn near_pops_out_and_far_sinks_in() {
        let mono = ctx();
        let conv = 5.0;
        let (l, r) = (mono.with_eye(-0.1, conv), mono.with_eye(0.1, conv));
        let view = mono.camera().view();
        let fwd = Vec3::new(view.m[2][0], view.m[2][1], view.m[2][2]);
        let at = |z: f32| mono.camera().eye + fwd * z;
        // Near: the right eye's copy is left of the left eye's (crossed).
        let (a, b) = (screen(&l, at(2.0)), screen(&r, at(2.0)));
        assert!(b.x < a.x, "near {a:?} {b:?}");
        // Far: uncrossed.
        let (a, b) = (screen(&l, at(40.0)), screen(&r, at(40.0)));
        assert!(b.x > a.x, "far {a:?} {b:?}");
        // The magnitude is the closed form's.
        let want = parallax_px(2.0, 0.2, mono.focal(), conv);
        let (a, b) = (screen(&l, at(2.0)), screen(&r, at(2.0)));
        assert!((b.x - a.x - want).abs() < 1e-2, "{} vs {want}", b.x - a.x);
    }

    #[test]
    fn the_eyes_share_every_screen_row() {
        let mono = ctx();
        let (l, r) = (mono.with_eye(-0.3, 3.0), mono.with_eye(0.3, 3.0));
        for p in [
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(3.0, -1.0, 4.0),
            Vec3::new(-2.0, 2.5, 10.0),
        ] {
            let (a, b, m) = (screen(&l, p), screen(&r, p), screen(&mono, p));
            assert!((a.y - b.y).abs() < 1e-3 && (a.y - m.y).abs() < 1e-3);
        }
    }

    #[test]
    fn project_eye_matches_the_shifted_context() {
        let mono = ctx();
        let c = mono.with_eye(0.25, 6.0);
        let p = Vec3::new(0.3, 1.0, 1.5);
        let v = mono.view_point(p);
        let a = project_eye(v, mono.focal(), mono.center(), 0.25, 6.0);
        let b = screen(&c, p);
        assert!((a.x - b.x).abs() < 1e-3 && (a.y - b.y).abs() < 1e-3);
    }

    #[test]
    fn a_zero_shift_keeps_the_mono_context_exactly() {
        let mono = ctx();
        assert_eq!(mono.with_eye(0.0, 4.0).focal(), mono.focal());
        assert_eq!(mono.with_eye(0.0, 4.0).center(), mono.center());
        let p = Vec3::new(0.25, 0.5, 2.0);
        assert_eq!(mono.with_eye(0.0, 4.0).view_point(p), mono.view_point(p));
    }

    #[test]
    fn eye_camera_moves_along_the_right_vector() {
        let cam = Camera3 {
            eye: Vec3::new(0.0, 0.0, -5.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        let r = eye_camera(&cam, 0.5);
        assert!((r.eye.x.abs() - 0.5).abs() < 1e-5 && r.eye.y == 0.0);
        assert_eq!(r.target - r.eye, cam.target - cam.eye);
    }

    #[test]
    fn resolve_follows_the_slider_screen_and_policy() {
        let s = StereoSettings::new(0.2, 4.0);
        assert_eq!(
            StereoFrame::resolve(None, true, 1.0, true).mono,
            StereoMono::Off
        );
        assert_eq!(
            StereoFrame::resolve(Some(&StereoSettings::OFF), true, 1.0, true).mono,
            StereoMono::Off
        );
        assert_eq!(
            StereoFrame::resolve(Some(&s), false, 1.0, true).mono,
            StereoMono::NoScreen
        );
        assert_eq!(
            StereoFrame::resolve(Some(&s), true, 0.0, true).mono,
            StereoMono::Slider
        );
        assert_eq!(
            StereoFrame::resolve(Some(&s), true, 1.0, false).mono,
            StereoMono::Budget
        );
        let always = StereoSettings {
            policy: StereoPolicy::Always,
            ..s
        };
        assert!(StereoFrame::resolve(Some(&always), true, 1.0, false).is_stereo());
        let f = StereoFrame::resolve(Some(&s), true, 0.5, true);
        assert!(f.is_stereo());
        assert!((f.separation - 0.1).abs() < 1e-6);
        assert_eq!(f.eye_shift(Eye::Left), -0.05);
        // A fixed slider overrides the hardware's.
        let f = StereoFrame::resolve(Some(&s.with_slider(1.0)), true, 0.0, true);
        assert!((f.separation - 0.2).abs() < 1e-6);
        assert_eq!(f.stats(3).slider_pct, 100);
        let f = StereoFrame::resolve(Some(&s.with_slider(f32::NAN)), true, 1.0, true);
        assert_eq!(f.mono, StereoMono::Slider);
    }

    #[test]
    fn layer_depth_pops_out_with_crossed_parallax() {
        let d = LayerDepth(8);
        assert_eq!(d.shift_px(Eye::Left, 1.0), 4.0);
        assert_eq!(d.shift_px(Eye::Right, 1.0), -4.0);
        assert_eq!(d.shift_px(Eye::Mono, 1.0), 0.0);
        assert_eq!(LayerDepth(-6).shift_px(Eye::Left, 0.5), -1.5);
        assert_eq!(LayerDepth::SCREEN.shift_px(Eye::Right, 1.0), 0.0);
    }

    #[test]
    fn the_governor_drops_after_three_slow_frames_and_retries() {
        let mut g = StereoGovernor::default();
        g.frame(true, 20_000, 16_667);
        g.frame(true, 20_000, 16_667);
        assert!(g.allows());
        g.frame(true, 10_000, 16_667); // a fast one resets the count
        for _ in 0..2 {
            g.frame(true, 20_000, 16_667);
        }
        assert!(g.allows());
        g.frame(true, 20_000, 16_667);
        assert!(!g.allows());
        for _ in 0..StereoGovernor::RETRY {
            assert!(!g.allows());
            g.frame(false, 5_000, 16_667);
        }
        assert!(g.allows());
    }

    #[test]
    fn stats_write_the_frame_field() {
        let mut s = heapless_string();
        StereoFrame::resolve(
            Some(&StereoSettings::new(0.1, 3.0).with_slider(0.57)),
            true,
            0.0,
            true,
        )
        .stats(4)
        .write(&mut s)
        .unwrap();
        assert_eq!(s, "stereo=on@57%");
        let mut s = heapless_string();
        StereoFrame::mono(StereoMono::Budget)
            .stats(0)
            .write(&mut s)
            .unwrap();
        assert_eq!(s, "stereo=budget");
    }

    fn heapless_string() -> String {
        String::new()
    }

    /// Records calls like a backend does and replays them.
    #[derive(Default)]
    struct Log {
        calls: Vec<String>,
        caps: Option<crate::Caps>,
    }

    impl Renderer for Log {
        fn caps(&self) -> &crate::Caps {
            self.caps.as_ref().unwrap_or(&crate::Caps::N3DS)
        }
        fn begin_frame(&mut self, _: Color) {}
        fn end_frame(&mut self) {}
        fn draw_sprite(&mut self, s: SpriteId, p: Vec2, _: &SpriteParams) {
            self.calls.push(format!("sprite {} {:?}", s.0, p));
        }
        fn fill_rect(&mut self, r: Rect, _: Color) {
            self.calls.push(format!("rect {:?}", r.pos));
        }
        fn draw_text(&mut self, _: FontId, _: Vec2, t: &str, _: &TextParams) {
            self.calls.push(format!("text {t}"));
        }
        fn set_layer_depth(&mut self, d: LayerDepth) {
            self.calls.push(format!("depth {}", d.0));
        }
        fn set_local_lights(&mut self, l: &[LocalLight]) {
            self.calls.push(format!("lights {}", l.len()));
        }
        fn draw_model(&mut self, _: ModelId, _: &crate::Transform3, _: Material, _: &ModelParams) {}
        fn draw_model_matrix(&mut self, m: ModelId, _: &Mat34, _: Material, _: &ModelParams) {
            self.calls.push(format!("model {}", m.0));
        }
    }

    #[test]
    fn replay_plays_every_call_in_order_with_its_slices() {
        let lights = [LocalLight::point(Vec3::ZERO, Color::WHITE, 1.0); 3];
        let text = b"xxHUD";
        let cmds = [
            StereoCmd::LocalLights(Span { start: 1, len: 2 }),
            StereoCmd::Model {
                model: ModelId(7),
                matrix: Mat34::IDENTITY,
                material: Material::VertexLit,
                params: ModelParams::DEFAULT,
            },
            StereoCmd::Depth(LayerDepth(4)),
            StereoCmd::Text {
                font: FontId(1),
                pos: Vec2::ZERO,
                text: Span { start: 2, len: 3 },
                params: TextParams::DEFAULT,
            },
            StereoCmd::Rect {
                rect: Rect::new(1.0, 2.0, 3.0, 4.0),
                color: Color::WHITE,
            },
        ];
        let mut log = Log::default();
        let data = StereoData {
            text,
            lights: &lights,
            ..StereoData::default()
        };
        replay(&mut log, &cmds, &data);
        assert_eq!(
            log.calls,
            [
                "lights 2",
                "model 7",
                "depth 4",
                "text HUD",
                "rect Vec2 { x: 1.0, y: 2.0 }"
            ]
        );
        assert!(log.caps().stereo);
    }
}
