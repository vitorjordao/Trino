//! The engine's achievement overlay: an unlock popup
//! ([`AchievementToast`]) and the list screen ([`AchievementList`]). Both
//! draw with `fill_rect`, `draw_sprite` and `draw_text` only, so every
//! backend shows them the same; text comes from the game's string tables
//! ([`UiStrings`]) by the table's keys, the key itself where no table has
//! it.
//!
//! Engine labels have keys too, with English fallbacks: `ach.title`
//! ("Achievements"), `ach.unlocked` ("Achievement unlocked"), `ach.hidden`
//! ("???") and `ach.hidden_desc` ("Hidden achievement").

use core::fmt::Write as _;

use super::{AchievementState, AchievementTable};
use crate::input::{Button, InputState};
use crate::math::{Color, Rect, Vec2};
use crate::platform::Platform;
use crate::render::{FontId, Renderer, Screen, SpriteParams, TextAlign, TextParams};
use crate::text::TextBuf;
use crate::ui::UiStrings;

/// How the overlay looks.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct OverlayStyle {
    pub font: FontId,
    pub background: Color,
    pub panel: Color,
    pub text: Color,
    /// Descriptions and locked rows.
    pub dim: Color,
    /// The medal, the focus frame, progress bars.
    pub accent: Color,
    /// The icons' source size in pixels (they are drawn at the row's
    /// height).
    pub icon_px: f32,
    /// Text size on top of the screen's whole-pixel fit (the player's
    /// `AccessSettings::text_scale` / 100).
    pub text_factor: f32,
    /// No slide: the popup appears and goes (`AccessSettings::reduce_motion`).
    pub reduce_motion: bool,
}

impl OverlayStyle {
    /// The engine's 5×7 font, white on near-black panels, gold accents.
    pub const DEFAULT: OverlayStyle = OverlayStyle {
        font: FontId::from_path("fonts/trino5x7"),
        background: Color::rgba(0, 0, 0, 200),
        panel: Color::rgba(28, 30, 44, 235),
        text: Color::WHITE,
        dim: Color::rgb(150, 156, 176),
        accent: Color::rgb(255, 196, 40),
        icon_px: 16.0,
        text_factor: 1.0,
        reduce_motion: false,
    };

    /// The player's text scale and motion setting.
    pub fn with_access(mut self, access: &crate::AccessSettings) -> OverlayStyle {
        self.text_factor = access.text_scale as f32 / 100.0;
        self.reduce_motion = access.reduce_motion;
        self
    }

    /// Text pixels for a `screen_h` screen: a whole pixel per 240 lines,
    /// times the text factor, rounded (bitmap fonts stay crisp).
    pub fn text_px(&self, screen_h: f32) -> f32 {
        let base = ((screen_h / 240.0) as i32).max(1) as f32;
        ((base * self.text_factor) + 0.5) as i32 as f32
    }
}

impl Default for OverlayStyle {
    fn default() -> Self {
        Self::DEFAULT
    }
}

fn label<'a>(strings: &'a dyn UiStrings, key: &'a str, fallback: &'a str) -> &'a str {
    strings.get(key).unwrap_or(fallback)
}

fn text<'a>(strings: &'a dyn UiStrings, key: &'a str) -> &'a str {
    strings.get(key).unwrap_or(key)
}

/// A medal: a gold disc on a ribbon, drawn with rectangles (no sprite
/// needed), `size` pixels square at `at`.
fn medal(renderer: &mut dyn Renderer, at: Vec2, size: f32, color: Color) {
    let u = (size / 8.0).max(1.0);
    let ribbon = Color::rgb(200, 60, 60);
    renderer.fill_rect(Rect::new(at.x + 2.0 * u, at.y, u, 3.0 * u), ribbon);
    renderer.fill_rect(Rect::new(at.x + 5.0 * u, at.y, u, 3.0 * u), ribbon);
    renderer.fill_rect(
        Rect::new(at.x + 2.0 * u, at.y + 3.0 * u, 4.0 * u, 5.0 * u),
        color,
    );
    renderer.fill_rect(Rect::new(at.x + u, at.y + 4.0 * u, 6.0 * u, 3.0 * u), color);
}

/// An achievement's picture: its icon, else the medal; dimmed when locked.
fn picture(
    renderer: &mut dyn Renderer,
    style: &OverlayStyle,
    icon: Option<crate::render::SpriteId>,
    at: Vec2,
    size: f32,
    unlocked: bool,
) {
    let tint = if unlocked {
        Color::WHITE
    } else {
        Color::rgb(70, 70, 80)
    };
    match icon {
        Some(sprite) => {
            let k = size / style.icon_px.max(1.0);
            renderer.draw_sprite(
                sprite,
                at,
                &SpriteParams {
                    scale: Vec2::new(k, k),
                    tint,
                    ..SpriteParams::DEFAULT
                },
            );
        }
        None => {
            let c = if unlocked { style.accent } else { style.dim };
            medal(renderer, at, size, c);
        }
    }
}

/// How long a popup stays, seconds (slides included).
pub const TOAST_SECONDS: f32 = 3.0;
const SLIDE: f32 = 0.25;
const TOAST_QUEUE: usize = 4;

/// The unlock popup: call [`update`](Self::update) every frame (it takes
/// the platform's new unlocks) and [`draw`](Self::draw) after the HUD. A
/// store that shows its own popups (Steam) gets none from here.
#[derive(Clone, Copy, Debug)]
pub struct AchievementToast {
    queue: [u8; TOAST_QUEUE],
    len: u8,
    /// Seconds the front popup has been shown.
    t: f32,
}

impl AchievementToast {
    pub const fn new() -> AchievementToast {
        AchievementToast {
            queue: [0; TOAST_QUEUE],
            len: 0,
            t: 0.0,
        }
    }

    /// Show the achievement at table `index` (after those queued).
    pub fn push(&mut self, index: usize) {
        if (self.len as usize) < TOAST_QUEUE {
            self.queue[self.len as usize] = index as u8;
            self.len += 1;
        }
    }

    /// The table index on screen now.
    pub fn current(&self) -> Option<usize> {
        (self.len > 0).then_some(self.queue[0] as usize)
    }

    /// Take the platform's unlocks and advance the clock.
    pub fn update(&mut self, platform: &mut dyn Platform, dt: f32) {
        let store = platform.services_info().store_toasts;
        while let Some(i) = platform.next_unlock() {
            if !store {
                self.push(i);
            }
        }
        self.advance(dt);
    }

    /// Advance the clock by `dt` seconds (no platform: tests, previews).
    pub fn advance(&mut self, dt: f32) {
        if self.len == 0 {
            return;
        }
        self.t += dt;
        if self.t >= TOAST_SECONDS {
            self.queue.copy_within(1.., 0);
            self.len -= 1;
            self.t = 0.0;
        }
    }

    /// The popup's box on a `size` screen inside `safe` margins, and how far
    /// it has slid in (0..=1).
    pub fn rect(&self, size: Vec2, px: f32, style: &OverlayStyle) -> (Rect, f32) {
        let line = 8.0 * px;
        let pad = 3.0 * px;
        let icon = 2.0 * line;
        let w = (size.x * 0.6).clamp((140.0 * px).min(size.x - 8.0), size.x - 8.0);
        let h = icon + 2.0 * pad;
        let shown = if style.reduce_motion {
            1.0
        } else {
            let into = (self.t / SLIDE).min(1.0);
            let out = ((TOAST_SECONDS - self.t) / SLIDE).min(1.0);
            into.min(out).max(0.0)
        };
        let top = 12.0 * px.min(2.0);
        let y = -h + (top + h) * shown;
        (Rect::new((size.x - w) * 0.5, y, w, h), shown)
    }

    pub fn draw(
        &self,
        renderer: &mut dyn Renderer,
        strings: &dyn UiStrings,
        table: &AchievementTable<'_>,
        style: &OverlayStyle,
    ) {
        let Some(info) = self.current().and_then(|i| table.get(i)) else {
            return;
        };
        let Some(size) = renderer.screen_size(Screen::Top) else {
            return;
        };
        let px = style.text_px(size.y);
        let font = strings.font(style.font);
        let (r, _) = self.rect(size, px, style);
        let line = 8.0 * px;
        let pad = 3.0 * px;
        renderer.fill_rect(r, style.panel);
        renderer.fill_rect(
            Rect::new(r.pos.x, r.pos.y + r.size.y - px, r.size.x, px),
            style.accent,
        );
        let icon = 2.0 * line;
        picture(
            renderer,
            style,
            info.icon,
            Vec2::new(r.pos.x + pad, r.pos.y + pad),
            icon,
            true,
        );
        let x = r.pos.x + 2.0 * pad + icon;
        let params = |tint| TextParams {
            scale: px,
            tint,
            align: TextAlign::Left,
        };
        renderer.draw_text(
            font,
            Vec2::new(x, r.pos.y + pad),
            label(strings, "ach.unlocked", "Achievement unlocked"),
            &params(style.accent),
        );
        renderer.draw_text(
            font,
            Vec2::new(x, r.pos.y + pad + line + px),
            text(strings, info.name),
            &params(style.text),
        );
    }
}

impl Default for AchievementToast {
    fn default() -> Self {
        Self::new()
    }
}

/// The list screen: every achievement with its picture, name,
/// description and, for one on a stat, a progress bar; hidden ones read
/// `???` until unlocked. Up/Down move the focus (the list scrolls), and
/// the game closes it on B.
#[derive(Clone, Copy, Debug, Default)]
pub struct AchievementList {
    pub focus: usize,
    pub scroll: usize,
    prev: InputState,
}

impl AchievementList {
    pub const fn new() -> AchievementList {
        AchievementList {
            focus: 0,
            scroll: 0,
            prev: InputState::IDLE,
        }
    }

    /// Move the focus with the D-pad (or the stick, as the UI does).
    pub fn update(&mut self, input: &InputState, table: &AchievementTable<'_>) {
        let n = table.len();
        let pressed = |b| input.just_pressed(&self.prev, b);
        if n > 0 {
            if pressed(Button::DpadDown) {
                self.focus = (self.focus + 1).min(n - 1);
            }
            if pressed(Button::DpadUp) {
                self.focus = self.focus.saturating_sub(1);
            }
        }
        self.prev = *input;
    }

    /// Rows that fit a `size` screen at `px`, and the row height.
    pub fn rows(size: Vec2, px: f32) -> (usize, f32) {
        let line = 8.0 * px;
        let row = 2.0 * line + 4.0 * px;
        let header = line + 6.0 * px;
        let room = size.y - header - 8.0 * px;
        (((room / row) as usize).max(1), row)
    }

    pub fn draw(
        &mut self,
        renderer: &mut dyn Renderer,
        strings: &dyn UiStrings,
        table: &AchievementTable<'_>,
        state: &AchievementState,
        style: &OverlayStyle,
    ) {
        let Some(size) = renderer.screen_size(renderer.screen()) else {
            return;
        };
        let safe = renderer.safe_area(renderer.screen());
        let (sx, sy) = (safe.left as f32, safe.top as f32);
        let inner = Vec2::new(
            size.x - sx - safe.right as f32,
            size.y - sy - safe.bottom as f32,
        );
        let px = style.text_px(size.y);
        let font = strings.font(style.font);
        let line = 8.0 * px;
        let pad = 2.0 * px;
        renderer.fill_rect(Rect::new(0.0, 0.0, size.x, size.y), style.background);
        let params = |tint, align| TextParams {
            scale: px,
            tint,
            align,
        };

        // Header: the title and the count.
        let mut count = TextBuf::<16>::new();
        let _ = write!(count, "{}/{}", state_count(state, table), table.len());
        renderer.draw_text(
            font,
            Vec2::new(sx + pad, sy + pad),
            label(strings, "ach.title", "Achievements"),
            &params(style.accent, TextAlign::Left),
        );
        renderer.draw_text(
            font,
            Vec2::new(sx + inner.x - pad, sy + pad),
            count.as_str(),
            &params(style.text, TextAlign::Right),
        );

        let (fit, row) = Self::rows(inner, px);
        let n = table.len();
        if n == 0 {
            return;
        }
        self.focus = self.focus.min(n - 1);
        if self.focus < self.scroll {
            self.scroll = self.focus;
        }
        if self.focus >= self.scroll + fit {
            self.scroll = self.focus + 1 - fit;
        }
        let top = sy + line + 6.0 * px;
        for (slot, i) in (self.scroll..n.min(self.scroll + fit)).enumerate() {
            let Some(a) = table.get(i) else { continue };
            let unlocked = state.is_unlocked(i);
            let y = top + slot as f32 * row;
            let r = Rect::new(sx + pad, y, inner.x - 2.0 * pad, row - 2.0 * px);
            renderer.fill_rect(r, style.panel);
            if i == self.focus {
                let a = style.accent;
                renderer.fill_rect(Rect::new(r.pos.x, r.pos.y, px, r.size.y), a);
                renderer.fill_rect(Rect::new(r.pos.x + r.size.x - px, r.pos.y, px, r.size.y), a);
            }
            let icon = 2.0 * line;
            picture(
                renderer,
                style,
                a.icon,
                Vec2::new(r.pos.x + pad, r.pos.y + px),
                icon,
                unlocked,
            );
            let x = r.pos.x + 2.0 * pad + icon;
            let secret = a.hidden && !unlocked;
            let (name, desc) = if secret {
                (
                    label(strings, "ach.hidden", "???"),
                    label(strings, "ach.hidden_desc", "Hidden achievement"),
                )
            } else {
                (text(strings, a.name), text(strings, a.description))
            };
            let tint = if unlocked { style.text } else { style.dim };
            renderer.draw_text(
                font,
                Vec2::new(x, r.pos.y + px),
                name,
                &params(tint, TextAlign::Left),
            );
            renderer.draw_text(
                font,
                Vec2::new(x, r.pos.y + px + line),
                desc,
                &params(style.dim, TextAlign::Left),
            );
            // Progress toward a stat threshold.
            if let Some(s) = a.stat
                && !unlocked
                && !secret
                && a.threshold > 0
            {
                let v = state.stat(s).clamp(0, a.threshold);
                let bar_w = 40.0 * px;
                let bx = r.pos.x + r.size.x - bar_w - 2.0 * pad;
                let by = r.pos.y + px;
                renderer.fill_rect(Rect::new(bx, by, bar_w, 2.0 * px), Color::rgb(60, 60, 70));
                let k = v as f32 / a.threshold as f32;
                renderer.fill_rect(Rect::new(bx, by, bar_w * k, 2.0 * px), style.accent);
                let mut t = TextBuf::<24>::new();
                let _ = write!(t, "{v}/{}", a.threshold);
                renderer.draw_text(
                    font,
                    Vec2::new(bx + bar_w, by + 3.0 * px),
                    t.as_str(),
                    &params(style.dim, TextAlign::Right),
                );
            }
            if unlocked {
                // A check mark at the row's right end.
                let c = style.accent;
                let (cx, cy) = (
                    r.pos.x + r.size.x - 2.0 * pad - 6.0 * px,
                    r.pos.y + 3.0 * px,
                );
                renderer.fill_rect(Rect::new(cx, cy + 3.0 * px, px, 2.0 * px), c);
                renderer.fill_rect(Rect::new(cx + px, cy + 4.0 * px, px, 2.0 * px), c);
                for k in 0..4 {
                    let k = k as f32;
                    renderer.fill_rect(
                        Rect::new(cx + (2.0 + k) * px, cy + (3.0 - k) * px, px, 2.0 * px),
                        c,
                    );
                }
            }
        }
        // A scroll bar when not every row fits.
        if n > fit {
            let track = fit as f32 * row;
            let h = track * fit as f32 / n as f32;
            let y = top + track * self.scroll as f32 / n as f32;
            let x = sx + inner.x - px;
            renderer.fill_rect(Rect::new(x, top, px, track), Color::rgb(60, 60, 70));
            renderer.fill_rect(Rect::new(x, y, px, h), style.accent);
        }
    }
}

fn state_count(state: &AchievementState, table: &AchievementTable<'_>) -> usize {
    (0..table.len()).filter(|&i| state.is_unlocked(i)).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn toast_queues_slides_and_expires() {
        let mut t = AchievementToast::new();
        let style = OverlayStyle::DEFAULT;
        assert_eq!(t.current(), None);
        t.push(2);
        t.push(0);
        assert_eq!(t.current(), Some(2));
        let size = Vec2::new(320.0, 240.0);
        let (r0, s0) = t.rect(size, 1.0, &style);
        assert_eq!(s0, 0.0);
        assert!(r0.pos.y + r0.size.y <= 0.0, "starts above the screen");
        t.advance(1.0);
        let (r1, s1) = t.rect(size, 1.0, &style);
        assert_eq!(s1, 1.0);
        assert!(r1.pos.y > 0.0 && r1.pos.x > 0.0 && r1.pos.x + r1.size.x < 320.0);
        t.advance(2.1);
        assert_eq!(t.current(), Some(0), "the next one after 3 s");
        // Reduce motion: no slide.
        let calm = OverlayStyle {
            reduce_motion: true,
            ..style
        };
        assert_eq!(t.rect(size, 1.0, &calm).1, 1.0);
        for _ in 0..8 {
            t.push(1);
        }
        assert_eq!(t.len as usize, TOAST_QUEUE, "the queue is bounded");
    }

    #[test]
    fn list_rows_fit_and_focus_moves() {
        let (fit, row) = AchievementList::rows(Vec2::new(288.0, 216.0), 1.0);
        assert!(fit >= 8 && row == 20.0, "{fit} rows of {row}");
        let (fit2, _) = AchievementList::rows(Vec2::new(288.0, 216.0), 2.0);
        assert!(fit2 < fit, "bigger text, fewer rows");
        let blob = super::super::table::tests::fixture();
        let table = AchievementTable::parse(&blob).unwrap();
        let mut l = AchievementList::new();
        let mut down = InputState::IDLE;
        down.set(Button::DpadDown, true);
        for _ in 0..5 {
            l.update(&down, &table);
            l.update(&InputState::IDLE, &table);
        }
        assert_eq!(l.focus, 2, "stops at the last");
        let mut up = InputState::IDLE;
        up.set(Button::DpadUp, true);
        l.update(&up, &table);
        assert_eq!(l.focus, 1);
    }
}
