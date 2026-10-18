//! A ready-made "Graphics" options page (issue #55): what a PC or Web
//! build offers the player over [`Platform::quality`] and its siblings —
//! the preset, the render scale, dynamic resolution, the frame cap, MSAA,
//! ambient occlusion and "Detect again" — with the calibration's pick and
//! the live 3D scale under them. Where the quality is fixed (a console)
//! it shows the preset and offers nothing else.
//!
//! The d-pad (the arrow keys) drives it: up and down pick a row, left and
//! right change it, A changes it too or runs an action row, B closes. It
//! draws with the game's font, in a panel wherever the game puts it:
//!
//! ```ignore
//! // update_with: open it from the game's menu, then let it run.
//! if input.just_pressed(&self.prev, Button::Select) {
//!     self.graphics.open();
//! }
//! if self.graphics.is_open() {
//!     self.graphics.update(platform, input, &self.prev);
//! }
//! // render, after the HUD:
//! self.graphics.draw(renderer, FONT, Vec2::new(40.0, 30.0), &OptionsStyle::DEFAULT);
//! ```
//!
//! A change asks the host ([`Platform::set_quality`]), which applies it
//! after the frame and saves it as the player's choice; the page shows
//! what the platform answers, so it is never out of step with the host.

use core::fmt::Write as _;

use crate::math::{Color, Rect, Vec2};
use crate::platform::Platform;
use crate::quality::{AoSetting, DynRes, FrameLimit, QualityInfo, QualityPreset, QualitySettings};
use crate::render::{FontId, Renderer, TextAlign, TextParams};
use crate::text::TextBuf;
use crate::{Button, InputState};

/// A row of the page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionsRow {
    Preset,
    RenderScale,
    DynamicResolution,
    FrameLimit,
    Msaa,
    AmbientOcclusion,
    /// Run the calibration again ([`Platform::detect_quality`]).
    Detect,
    /// Close the page.
    Back,
}

impl OptionsRow {
    /// Where the host offers the presets (PC, Web).
    pub const HOSTED: [OptionsRow; 8] = [
        OptionsRow::Preset,
        OptionsRow::RenderScale,
        OptionsRow::DynamicResolution,
        OptionsRow::FrameLimit,
        OptionsRow::Msaa,
        OptionsRow::AmbientOcclusion,
        OptionsRow::Detect,
        OptionsRow::Back,
    ];
    /// Where the quality is fixed (the consoles).
    pub const FIXED: [OptionsRow; 2] = [OptionsRow::Preset, OptionsRow::Back];

    pub const fn label(self) -> &'static str {
        match self {
            OptionsRow::Preset => "Quality",
            OptionsRow::RenderScale => "Render scale",
            OptionsRow::DynamicResolution => "Dynamic resolution",
            OptionsRow::FrameLimit => "Frame limit",
            OptionsRow::Msaa => "Anti-aliasing",
            OptionsRow::AmbientOcclusion => "Ambient occlusion",
            OptionsRow::Detect => "Detect again",
            OptionsRow::Back => "Back",
        }
    }
}

/// What a frame of [`GraphicsOptions::update`] did.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum OptionsEvent {
    None,
    /// New settings were asked for.
    Changed,
    /// The calibration was asked for.
    Detect,
    /// The page closed (B, or Back).
    Closed,
}

/// How the page looks.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OptionsStyle {
    pub title: &'static str,
    /// The text's pixel scale.
    pub scale: f32,
    /// A row's height, px.
    pub line: f32,
    /// The panel's width, px.
    pub width: f32,
    pub panel: Color,
    pub text: Color,
    /// Values, the info lines.
    pub dim: Color,
    /// The picked row.
    pub accent: Color,
}

impl OptionsStyle {
    /// White on a dark panel, 1:1 text (a 5×7 font: 260 px wide).
    pub const DEFAULT: OptionsStyle = OptionsStyle {
        title: "Graphics",
        scale: 1.0,
        line: 12.0,
        width: 260.0,
        panel: Color::rgba(12, 16, 28, 220),
        text: Color::WHITE,
        dim: Color::rgb(170, 180, 200),
        accent: Color::rgb(255, 214, 90),
    };
}

/// The page's state: open or not, the picked row, and what the platform
/// last said (what [`draw`](Self::draw) shows).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct GraphicsOptions {
    open: bool,
    row: usize,
    shown: QualitySettings,
    info: Option<QualityInfo>,
}

impl Default for GraphicsOptions {
    fn default() -> Self {
        Self::new()
    }
}

impl GraphicsOptions {
    pub const fn new() -> Self {
        GraphicsOptions {
            open: false,
            row: 0,
            shown: QualitySettings::HIGH,
            info: None,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// Open on the first row.
    pub fn open(&mut self) {
        self.open = true;
        self.row = 0;
    }

    pub fn close(&mut self) {
        self.open = false;
    }

    /// The rows the page has on this platform (what it last saw).
    pub fn rows(&self) -> &'static [OptionsRow] {
        if self.info.is_some() {
            &OptionsRow::HOSTED
        } else {
            &OptionsRow::FIXED
        }
    }

    /// The picked row.
    pub fn row(&self) -> OptionsRow {
        let rows = self.rows();
        rows[self.row.min(rows.len() - 1)]
    }

    /// Pick `row` (tests, a pointer over the panel).
    pub fn select(&mut self, row: OptionsRow) {
        if let Some(i) = self.rows().iter().position(|r| *r == row) {
            self.row = i;
        }
    }

    /// One frame while open: read the platform, then this frame's buttons
    /// against `prev`'s.
    pub fn update(
        &mut self,
        platform: &mut dyn Platform,
        input: &InputState,
        prev: &InputState,
    ) -> OptionsEvent {
        self.shown = platform.quality();
        self.info = platform.quality_info();
        if !self.open {
            return OptionsEvent::None;
        }
        let pressed = |b| input.just_pressed(prev, b);
        let n = self.rows().len();
        self.row = self.row.min(n - 1);
        if pressed(Button::DpadUp) {
            self.row = (self.row + n - 1) % n;
        }
        if pressed(Button::DpadDown) {
            self.row = (self.row + 1) % n;
        }
        if pressed(Button::B) {
            self.open = false;
            return OptionsEvent::Closed;
        }
        let step = if pressed(Button::DpadLeft) {
            -1
        } else if pressed(Button::DpadRight) || pressed(Button::A) {
            1
        } else {
            return OptionsEvent::None;
        };
        self.change(platform, step, pressed(Button::A))
    }

    /// Change the picked row by `step` (±1); `confirm` runs an action row.
    pub fn change(
        &mut self,
        platform: &mut dyn Platform,
        step: i32,
        confirm: bool,
    ) -> OptionsEvent {
        let row = self.row();
        match row {
            OptionsRow::Back if confirm => {
                self.open = false;
                return OptionsEvent::Closed;
            }
            OptionsRow::Detect if confirm => {
                platform.detect_quality();
                return OptionsEvent::Detect;
            }
            OptionsRow::Back | OptionsRow::Detect => return OptionsEvent::None,
            _ => {}
        }
        if self.info.is_none() {
            return OptionsEvent::None;
        }
        let q = self.shown;
        let next = match row {
            OptionsRow::Preset => {
                let named = QualityPreset::NAMED;
                let at = named.iter().position(|p| *p == q.preset).unwrap_or(2) as i32;
                let p = named[(at + step).rem_euclid(named.len() as i32) as usize];
                // A preset keeps the player's frame cap (a display choice).
                QualitySettings {
                    frame_limit: q.frame_limit,
                    ..platform
                        .quality_preset(p)
                        .unwrap_or(QualitySettings::preset(p))
                }
            }
            OptionsRow::RenderScale => {
                // Snapped to the 5% steps (no_std: no `round`).
                let s = ((q.render_scale * 20.0 + 0.5) as i32 + step) as f32 / 20.0;
                QualitySettings {
                    preset: QualityPreset::Custom,
                    render_scale: s.clamp(0.5, 2.0),
                    ..q
                }
            }
            OptionsRow::DynamicResolution => QualitySettings {
                preset: QualityPreset::Custom,
                dynamic_resolution: match q.dynamic_resolution {
                    Some(_) => None,
                    None => Some(
                        platform
                            .quality_preset(q.preset)
                            .and_then(|p| p.dynamic_resolution)
                            .unwrap_or(DynRes::DEFAULT),
                    ),
                },
                ..q
            },
            OptionsRow::FrameLimit => QualitySettings {
                frame_limit: cycle(&FrameLimit::CHOICES, q.frame_limit, step),
                ..q
            },
            OptionsRow::Msaa => QualitySettings {
                preset: QualityPreset::Custom,
                msaa: if q.msaa > 1 { 1 } else { 4 },
                ..q
            },
            OptionsRow::AmbientOcclusion => QualitySettings {
                preset: QualityPreset::Custom,
                ao: cycle(&AoSetting::ALL, q.ao, step),
                ..q
            },
            OptionsRow::Detect | OptionsRow::Back => q,
        };
        if next == q {
            return OptionsEvent::None;
        }
        platform.set_quality(next);
        // Shown at once; the host's answer replaces it next frame.
        self.shown = next;
        OptionsEvent::Changed
    }

    /// A row's value as the page shows it.
    pub fn value(&self, row: OptionsRow) -> TextBuf<24> {
        let q = &self.shown;
        let mut b = TextBuf::new();
        let _ = match row {
            OptionsRow::Preset if self.info.is_none() => write!(b, "{} (fixed)", q.preset.label()),
            OptionsRow::Preset => write!(b, "{}", q.preset.label()),
            OptionsRow::RenderScale => write!(b, "{}%", (q.render_scale * 100.0 + 0.5) as u32),
            OptionsRow::DynamicResolution => match q.dynamic_resolution {
                None => write!(b, "Off"),
                Some(d) => write!(
                    b,
                    "On {}-{}%",
                    (d.min_scale * 100.0 + 0.5) as u32,
                    (d.max_scale * 100.0 + 0.5) as u32
                ),
            },
            OptionsRow::FrameLimit => match q.frame_limit {
                FrameLimit::VSync => write!(b, "VSync"),
                FrameLimit::Fps(n) => write!(b, "{n} fps"),
                FrameLimit::Unlimited => write!(b, "Unlimited"),
            },
            OptionsRow::Msaa if q.msaa > 1 => write!(b, "{}x MSAA", q.msaa),
            OptionsRow::Msaa => write!(b, "Off"),
            OptionsRow::AmbientOcclusion => write!(b, "{}", q.ao.label()),
            OptionsRow::Detect | OptionsRow::Back => Ok(()),
        };
        b
    }

    /// The lines under the rows: the live 3D scale and GPU time, and the
    /// calibration's pick (`None` where the quality is fixed).
    pub fn info_lines(&self) -> Option<(TextBuf<40>, TextBuf<40>)> {
        let i = self.info?;
        let mut live = TextBuf::new();
        let _ = write!(live, "3D {}%", (i.scale * 100.0 + 0.5) as u32);
        if i.gpu_us > 0 {
            let _ = write!(
                live,
                "  GPU {}.{} ms",
                i.gpu_us / 1000,
                i.gpu_us % 1000 / 100
            );
        }
        let mut detected = TextBuf::new();
        let _ = match i.detected {
            Some(p) => write!(detected, "Detected: {} ({} GPU)", p.label(), i.gpu.key()),
            None => write!(detected, "Detect: not run ({} GPU)", i.gpu.key()),
        };
        Some((live, detected))
    }

    /// The panel's size with `style`.
    pub fn size(&self, style: &OptionsStyle) -> Vec2 {
        let lines = 1 + self.rows().len() + if self.info.is_some() { 2 } else { 0 };
        Vec2::new(style.width, (lines as f32 + 1.0) * style.line)
    }

    /// Draw the page at `pos` (its top-left) with `font`, when open.
    pub fn draw(&self, renderer: &mut dyn Renderer, font: FontId, pos: Vec2, style: &OptionsStyle) {
        if !self.open {
            return;
        }
        let size = self.size(style);
        renderer.fill_rect(Rect::new(pos.x, pos.y, size.x, size.y), style.panel);
        let pad = 8.0;
        let text = |tint, align| TextParams {
            scale: style.scale,
            tint,
            align,
        };
        let mut y = pos.y + style.line * 0.5;
        renderer.draw_text(
            font,
            Vec2::new(pos.x + pad, y),
            style.title,
            &text(style.text, TextAlign::Left),
        );
        y += style.line;
        let picked = self.row();
        for row in self.rows() {
            let on = *row == picked;
            if on {
                let accent = Color::rgba(style.accent.r, style.accent.g, style.accent.b, 48);
                renderer.fill_rect(
                    Rect::new(pos.x + 2.0, y - 2.0, size.x - 4.0, style.line),
                    accent,
                );
            }
            let tint = if on { style.accent } else { style.text };
            renderer.draw_text(
                font,
                Vec2::new(pos.x + pad, y),
                row.label(),
                &text(tint, TextAlign::Left),
            );
            let value = self.value(*row);
            renderer.draw_text(
                font,
                Vec2::new(pos.x + size.x - pad, y),
                value.as_str(),
                &text(if on { style.accent } else { style.dim }, TextAlign::Right),
            );
            y += style.line;
        }
        if let Some((live, detected)) = self.info_lines() {
            for line in [live.as_str(), detected.as_str()] {
                renderer.draw_text(
                    font,
                    Vec2::new(pos.x + pad, y),
                    line,
                    &text(style.dim, TextAlign::Left),
                );
                y += style.line;
            }
        }
    }
}

/// The value after `now` in `all` by `step` (wrapping; the first when
/// `now` isn't one).
fn cycle<T: Copy + PartialEq>(all: &[T], now: T, step: i32) -> T {
    let n = all.len() as i32;
    match all.iter().position(|v| *v == now) {
        Some(i) => all[(i as i32 + step).rem_euclid(n) as usize],
        None => all[0],
    }
}
