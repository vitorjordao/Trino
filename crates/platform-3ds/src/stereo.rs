//! Stereoscopic 3D on the top screen (issue #63, ADR 0032): the frame's
//! top-screen calls recorded while the 3D slider is up, replayed once per
//! eye by the renderer (`trino_core::stereo`).

use alloc::vec::Vec;

use trino_core::stereo::{
    Eye, LayerDepth, Span, StereoCmd, StereoData, StereoFrame, StereoGovernor, StereoMono,
    StereoSettings,
};
use trino_core::{Color, LocalLight, Ripple, ShadowPlane};

/// One frame's recorded calls and the arenas their slices live in. The
/// `Vec`s keep their capacity from frame to frame.
#[derive(Default)]
pub(crate) struct Recording {
    pub cmds: Vec<StereoCmd>,
    text: Vec<u8>,
    lights: Vec<LocalLight>,
    ripples: Vec<Ripple>,
    planes: Vec<ShadowPlane>,
}

fn span<T: Copy>(arena: &mut Vec<T>, items: &[T]) -> Span {
    let start = arena.len() as u32;
    arena.extend_from_slice(items);
    Span {
        start,
        len: items.len() as u32,
    }
}

impl Recording {
    pub fn clear(&mut self) {
        self.cmds.clear();
        self.text.clear();
        self.lights.clear();
        self.ripples.clear();
        self.planes.clear();
    }

    pub fn push(&mut self, cmd: StereoCmd) {
        self.cmds.push(cmd);
    }

    pub fn text(&mut self, s: &str) -> Span {
        span(&mut self.text, s.as_bytes())
    }

    pub fn lights(&mut self, l: &[LocalLight]) -> Span {
        span(&mut self.lights, l)
    }

    pub fn ripples(&mut self, r: &[Ripple]) -> Span {
        span(&mut self.ripples, r)
    }

    pub fn planes(&mut self, p: &[ShadowPlane]) -> Span {
        span(&mut self.planes, p)
    }

    pub fn data(&self) -> StereoData<'_> {
        StereoData {
            text: &self.text,
            lights: &self.lights,
            ripples: &self.ripples,
            planes: &self.planes,
        }
    }
}

/// The renderer's stereo state.
pub(crate) struct Stereo {
    /// This frame's: what `set_stereo` resolved to.
    pub frame: StereoFrame,
    /// Recording the top screen (a stereo frame not replayed yet).
    pub recording: bool,
    /// The eye being replayed (`Mono` outside a replay).
    pub eye: Eye,
    /// The depth of the next 2D (`set_layer_depth`).
    pub depth: LayerDepth,
    /// The top screen had a draw before `set_stereo`.
    pub top_drawn: bool,
    pub rec: Recording,
    pub governor: StereoGovernor,
    /// The frame's clear color (the right eye clears to it).
    pub clear: Color,
    /// The budget of the last stereo request, and when the last frame
    /// began (`now_us`): the governor's frame interval.
    pub budget_us: u32,
    pub last_begin: Option<u32>,
    /// What the shim's `gfxSet3D` is set to.
    pub shown: bool,
    /// A slider position in place of the hardware's (the self-test's
    /// stereo timings; `N3dsRenderer::set_stereo_slider_override`).
    pub slider_override: Option<f32>,
}

impl Stereo {
    pub fn new() -> Stereo {
        Stereo {
            frame: StereoFrame::mono(StereoMono::Off),
            recording: false,
            eye: Eye::Mono,
            depth: LayerDepth::SCREEN,
            top_drawn: false,
            rec: Recording::default(),
            governor: StereoGovernor::default(),
            clear: Color::BLACK,
            budget_us: StereoSettings::OFF.budget_us,
            last_begin: None,
            shown: false,
            slider_override: None,
        }
    }

    /// A new frame, `interval_us` after the last one began (`None` for the
    /// first): the governor learns whether the last frame fit.
    pub fn begin(&mut self, clear: Color, interval_us: Option<u32>) {
        if let Some(us) = interval_us {
            // A frame that missed its vsync takes a whole extra refresh:
            // a quarter of the budget of slack ignores jitter.
            let budget = self.budget_us.saturating_add(self.budget_us / 4);
            self.governor.frame(self.frame.is_stereo(), us, budget);
        }
        self.frame = StereoFrame::mono(StereoMono::Off);
        self.recording = false;
        self.eye = Eye::Mono;
        self.depth = LayerDepth::SCREEN;
        self.top_drawn = false;
        self.rec.clear();
        self.clear = clear;
    }

    /// This eye's 2D offset, pixels.
    pub fn shift_2d(&self) -> f32 {
        self.depth.shift_px(self.eye, self.frame.strength)
    }
}
