//! A small immediate-mode UI over `fill_rect` and bitmap text, in the
//! style of the reference art: dark navy panels with neon borders, slanted
//! headers, stat bars, a pentagon radar, shield crests, button hints.
//!
//! Layouts are written for a 320×240 design canvas and drawn at an
//! integer scale ([`Ui::scale`]), centered on wider pictures: the same code
//! fills the N64's 320×240, the 3DS's 400×240 and a PC window.
//!
//! No textures: on the N64 a sprite blit reloads TMEM every time, while a
//! rectangle is one fill command. Shapes that aren't rectangles (the radar,
//! crests, slants) are drawn as horizontal spans.

pub mod controls;
pub mod i18n;
pub mod screens;

use trino_core::{Color, FontId, Rect, Renderer, TextAlign, TextParams, Vec2};

pub const F_UI: FontId = FontId::from_path("fonts/se_ui");
/// The characters both fonts bake (`chars` in `assets/manifest.toml`; the
/// engine's `solid_eleven_assets` test keeps the two equal). Strings and
/// names are uppercased into this set.
pub const FONT_CHARS: &str =
    " ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789.,:;-+/%!?()'&#<>*=_ÁÂÃÀÇÉÊÍÓÔÕÚÜ";
pub const F_BIG: FontId = FontId::from_path("fonts/se_big");
pub const F_HUGE: FontId = FontId::from_path("fonts/se_huge");

/// The design canvas.
pub const DESIGN_W: f32 = 320.0;
pub const DESIGN_H: f32 = 240.0;

/// Palette of the reference UI.
pub mod pal {
    use trino_core::Color;
    pub const NAVY: Color = Color::rgb(18, 22, 52);
    pub const NAVY_2: Color = Color::rgb(28, 34, 74);
    pub const PANEL: Color = Color::rgba(16, 20, 46, 230);
    pub const INK: Color = Color::rgb(236, 240, 255);
    pub const DIM: Color = Color::rgb(150, 160, 196);
    pub const CYAN: Color = Color::rgb(40, 210, 255);
    pub const LIME: Color = Color::rgb(190, 255, 60);
    pub const YELLOW: Color = Color::rgb(255, 214, 40);
    pub const ORANGE: Color = Color::rgb(255, 140, 40);
    pub const GREEN: Color = Color::rgb(70, 210, 90);
    pub const RED: Color = Color::rgb(230, 50, 70);
    pub const BLUE: Color = Color::rgb(50, 110, 240);
    pub const PINK: Color = Color::rgb(240, 60, 150);
    pub const PURPLE: Color = Color::rgb(110, 60, 200);
    pub const SHADE: Color = Color::rgba(0, 0, 0, 120);
}

/// Text size: the 11 px UI font or the 22 px title font.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Size {
    Small,
    Big,
}

/// A frame's UI context.
pub struct Ui<'a> {
    pub r: &'a mut dyn Renderer,
    /// Pixels per design unit (an integer ≥ 1).
    pub scale: f32,
    /// Top-left of the design canvas in screen pixels.
    pub origin: Vec2,
    pub screen: Vec2,
}

impl<'a> Ui<'a> {
    /// Fit the design canvas into `screen`: exactly 1:1 on the consoles'
    /// screens (320×240, 400×240), scaled to fit a larger picture (the PC),
    /// in whole pixels per unit when that fills it within 15%, else a
    /// continuous scale.
    pub fn new(r: &'a mut dyn Renderer, screen: Vec2) -> Ui<'a> {
        let fit = (screen.x / DESIGN_W).min(screen.y / DESIGN_H).max(1.0);
        let whole = (fit as i32).max(1) as f32;
        let s = if fit - whole < fit * 0.15 { whole } else { fit };
        let origin = Vec2::new(
            ((screen.x - DESIGN_W * s) * 0.5).max(0.0) as i32 as f32,
            ((screen.y - DESIGN_H * s) * 0.5).max(0.0) as i32 as f32,
        );
        Ui {
            r,
            scale: s,
            origin,
            screen,
        }
    }

    /// Design units → screen pixels.
    pub fn px(&self, x: f32, y: f32) -> Vec2 {
        Vec2::new(
            self.origin.x + x * self.scale,
            self.origin.y + y * self.scale,
        )
    }

    pub fn rect(&self, r: Rect) -> Rect {
        let p = self.px(r.pos.x, r.pos.y);
        Rect {
            pos: p,
            size: Vec2::new(r.size.x * self.scale, r.size.y * self.scale),
        }
    }

    /// A filled rectangle in design units.
    pub fn fill(&mut self, r: Rect, c: Color) {
        let s = self.rect(r);
        self.r.fill_rect(s, c);
    }

    /// The whole screen (not just the canvas): backgrounds.
    pub fn fill_screen(&mut self, c: Color) {
        self.r
            .fill_rect(Rect::new(0.0, 0.0, self.screen.x, self.screen.y), c);
    }

    /// A 1-unit border around `r`.
    pub fn border(&mut self, r: Rect, c: Color, t: f32) {
        let Rect { pos, size } = r;
        self.fill(Rect::new(pos.x, pos.y, size.x, t), c);
        self.fill(Rect::new(pos.x, pos.y + size.y - t, size.x, t), c);
        self.fill(Rect::new(pos.x, pos.y, t, size.y), c);
        self.fill(Rect::new(pos.x + size.x - t, pos.y, t, size.y), c);
    }

    /// A panel: translucent navy, a colored rim.
    pub fn panel(&mut self, r: Rect, rim: Color) {
        self.fill(r, pal::PANEL);
        self.border(r, rim, 1.0);
    }

    /// A parallelogram slanted like the reference headers: `lean` design
    /// units of horizontal shift from bottom to top.
    pub fn slant(&mut self, r: Rect, c: Color, lean: f32) {
        let rows = (r.size.y * self.scale) as i32;
        if rows <= 0 {
            return;
        }
        // Spans of 2 px rows (cheap on the N64, smooth enough).
        let step = 2;
        let mut y = 0;
        while y < rows {
            let h = step.min(rows - y) as f32;
            let k = 1.0 - (y as f32 + h * 0.5) / rows as f32;
            let off = lean * k * self.scale;
            let p = self.px(r.pos.x, r.pos.y);
            self.r.fill_rect(
                Rect::new(p.x + off, p.y + y as f32, r.size.x * self.scale, h),
                c,
            );
            y += step;
        }
    }

    /// The outline of [`slant`](Self::slant), `t` design units thick: its
    /// sides lean with the shape, its top and bottom follow the first and
    /// last rows (a square border overshoots the slanted corners).
    pub fn slant_border(&mut self, r: Rect, c: Color, lean: f32, t: f32) {
        let rows = (r.size.y * self.scale) as i32;
        if rows <= 0 {
            return;
        }
        let p = self.px(r.pos.x, r.pos.y);
        let w = r.size.x * self.scale;
        let th = (t * self.scale).max(1.0);
        let step = 2;
        let off = |y: i32, h: f32| lean * (1.0 - (y as f32 + h * 0.5) / rows as f32) * self.scale;
        let mut y = 0;
        while y < rows {
            let h = step.min(rows - y) as f32;
            let o = off(y, h);
            self.r
                .fill_rect(Rect::new(p.x + o, p.y + y as f32, th, h), c);
            self.r
                .fill_rect(Rect::new(p.x + o + w - th, p.y + y as f32, th, h), c);
            y += step;
        }
        let top = off(0, step.min(rows) as f32);
        let last = (rows - 1) / step * step;
        let bottom = off(last, (rows - last) as f32);
        self.r.fill_rect(Rect::new(p.x + top, p.y, w, th), c);
        self.r
            .fill_rect(Rect::new(p.x + bottom, p.y + rows as f32 - th, w, th), c);
    }

    /// Text in design units. Small text at 2x scale uses the big font, so
    /// the PC's interface stays crisp.
    pub fn text(&mut self, x: f32, y: f32, s: &str, c: Color, size: Size, align: TextAlign) {
        let (font, k) = self.font_for(size);
        let p = self.px(x, y);
        self.r.draw_text(
            font,
            p,
            s,
            &TextParams {
                scale: k,
                tint: c,
                align,
            },
        );
    }

    /// Text with a 1-unit drop shadow (over the 3D picture).
    pub fn text_shadow(&mut self, x: f32, y: f32, s: &str, c: Color, size: Size, align: TextAlign) {
        self.text(x + 1.0, y + 1.0, s, pal::SHADE, size, align);
        self.text(x, y, s, c, size, align);
    }

    fn font_for(&self, size: Size) -> (FontId, f32) {
        match size {
            Size::Small if self.scale >= 3.0 => (F_HUGE, self.scale * 0.25),
            Size::Small if self.scale >= 1.5 => (F_BIG, self.scale * 0.5),
            Size::Small => (F_UI, self.scale),
            Size::Big if self.scale >= 2.0 => (F_HUGE, self.scale * 0.5),
            Size::Big => (F_BIG, self.scale),
        }
    }

    /// Width of `s` in design units.
    pub fn measure(&self, s: &str, size: Size) -> f32 {
        let (font, k) = self.font_for(size);
        self.r.measure_text(font, s, k).x / self.scale
    }

    /// A horizontal bar: track, fill (0..1) and value.
    pub fn bar(&mut self, r: Rect, value: f32, c: Color) {
        self.fill(r, Color::rgba(8, 10, 26, 220));
        let v = value.clamp(0.0, 1.0);
        if v > 0.0 {
            self.fill(Rect::new(r.pos.x, r.pos.y, r.size.x * v, r.size.y), c);
        }
    }

    /// A filled convex polygon (design units), drawn as horizontal spans.
    pub fn polygon(&mut self, pts: &[Vec2], c: Color) {
        if pts.len() < 3 {
            return;
        }
        let sp: [Vec2; 12] = core::array::from_fn(|i| {
            if i < pts.len() {
                self.px(pts[i].x, pts[i].y)
            } else {
                Vec2::ZERO
            }
        });
        let n = pts.len().min(12);
        let mut top = f32::MAX;
        let mut bottom = f32::MIN;
        for p in &sp[..n] {
            top = top.min(p.y);
            bottom = bottom.max(p.y);
        }
        let step = 2.0;
        let mut y = top as i32 as f32;
        while y < bottom {
            let yc = y + step * 0.5;
            let mut lo = f32::MAX;
            let mut hi = f32::MIN;
            for k in 0..n {
                let a = sp[k];
                let b = sp[(k + 1) % n];
                if (a.y <= yc && b.y > yc) || (b.y <= yc && a.y > yc) {
                    let x = a.x + (yc - a.y) / (b.y - a.y) * (b.x - a.x);
                    lo = lo.min(x);
                    hi = hi.max(x);
                }
            }
            if hi > lo {
                self.r.fill_rect(Rect::new(lo, y, hi - lo, step), c);
            }
            y += step;
        }
    }

    /// The reference's stat pentagon: `values` (0..1) on five spokes.
    pub fn radar(
        &mut self,
        cx: f32,
        cy: f32,
        radius: f32,
        values: [f32; 5],
        fill: Color,
        rim: Color,
    ) {
        // Spoke directions, the first straight up (unit circle, design y down).
        const DIRS: [(f32, f32); 5] = [
            (0.0, -1.0),
            (0.951, -0.309),
            (0.588, 0.809),
            (-0.588, 0.809),
            (-0.951, -0.309),
        ];
        let ring = |k: f32| -> [Vec2; 5] {
            core::array::from_fn(|i| {
                Vec2::new(cx + DIRS[i].0 * radius * k, cy + DIRS[i].1 * radius * k)
            })
        };
        self.polygon(&ring(1.0), rim);
        self.polygon(&ring(0.94), Color::rgba(20, 26, 60, 255));
        self.polygon(&ring(0.62), Color::rgba(30, 38, 80, 255));
        self.polygon(&ring(0.3), Color::rgba(38, 48, 96, 255));
        // The stat shape is not always convex: draw it as five triangles
        // from the center.
        let pts: [Vec2; 5] = core::array::from_fn(|i| {
            let v = values[i].clamp(0.05, 1.0);
            Vec2::new(cx + DIRS[i].0 * radius * v, cy + DIRS[i].1 * radius * v)
        });
        for i in 0..5 {
            self.polygon(&[Vec2::new(cx, cy), pts[i], pts[(i + 1) % 5]], fill);
        }
    }

    /// A club crest: the outline in `rim`, the field split in two colors,
    /// the club's three letters.
    pub fn crest(
        &mut self,
        x: f32,
        y: f32,
        w: f32,
        kind: crate::sim::team::Crest,
        a: Color,
        b: Color,
        label: &str,
    ) {
        use crate::sim::team::Crest;
        let h = w * 1.15;
        let outline: ([Vec2; 6], usize) = match kind {
            Crest::Shield => (
                [
                    Vec2::new(x, y),
                    Vec2::new(x + w, y),
                    Vec2::new(x + w, y + h * 0.55),
                    Vec2::new(x + w * 0.5, y + h),
                    Vec2::new(x, y + h * 0.55),
                    Vec2::new(x, y),
                ],
                5,
            ),
            Crest::Round => (
                [
                    Vec2::new(x + w * 0.3, y),
                    Vec2::new(x + w * 0.7, y),
                    Vec2::new(x + w, y + h * 0.4),
                    Vec2::new(x + w * 0.7, y + h * 0.9),
                    Vec2::new(x + w * 0.3, y + h * 0.9),
                    Vec2::new(x, y + h * 0.4),
                ],
                6,
            ),
            Crest::Diamond => (
                [
                    Vec2::new(x + w * 0.5, y),
                    Vec2::new(x + w, y + h * 0.45),
                    Vec2::new(x + w * 0.5, y + h * 0.95),
                    Vec2::new(x, y + h * 0.45),
                    Vec2::ZERO,
                    Vec2::ZERO,
                ],
                4,
            ),
        };
        let pts = &outline.0[..outline.1];
        self.polygon(pts, Color::rgb(240, 240, 246));
        // Inner field, inset by 1.5 units toward the center.
        let c = Vec2::new(x + w * 0.5, y + h * 0.45);
        let inset: [Vec2; 6] = core::array::from_fn(|i| {
            if i < pts.len() {
                let p = pts[i];
                let d = Vec2::new(c.x - p.x, c.y - p.y);
                let l = trino_core::math3d::sqrt(d.x * d.x + d.y * d.y).max(1e-3);
                Vec2::new(p.x + d.x / l * 1.6, p.y + d.y / l * 1.6)
            } else {
                Vec2::ZERO
            }
        });
        self.polygon(&inset[..pts.len()], a);
        // The right half in the second color.
        let mut right = [Vec2::ZERO; 6];
        let mut n = 0;
        for i in 0..pts.len() {
            let p = inset[i];
            if p.x >= c.x - 0.01 {
                right[n] = p;
                n += 1;
            }
        }
        if n >= 2 {
            let mut half = [Vec2::ZERO; 8];
            half[0] = Vec2::new(
                c.x,
                inset
                    .iter()
                    .take(pts.len())
                    .fold(f32::MAX, |m, p| m.min(p.y)),
            );
            let mut k = 1;
            for p in &right[..n] {
                half[k] = *p;
                k += 1;
            }
            half[k] = Vec2::new(
                c.x,
                inset
                    .iter()
                    .take(pts.len())
                    .fold(f32::MIN, |m, p| m.max(p.y)),
            );
            k += 1;
            self.polygon(&half[..k], b);
        }
        if !label.is_empty() && w >= 18.0 {
            self.text_shadow(
                x + w * 0.5,
                y + h * 0.28,
                label,
                pal::INK,
                Size::Small,
                TextAlign::Center,
            );
        }
    }

    /// A controller hint like the reference's footer: a colored pill with
    /// the button letter, then the action.
    pub fn hint(&mut self, x: f32, y: f32, letter: &str, c: Color, label: &str) -> f32 {
        let lw = self.measure(label, Size::Small);
        let w = 16.0 + lw + 6.0;
        self.slant(
            Rect::new(x, y, w, 13.0),
            Color::rgba(c.r / 3, c.g / 3, c.b / 3, 230),
            3.0,
        );
        self.slant_border(Rect::new(x, y, w, 13.0), c, 3.0, 1.0);
        self.fill(Rect::new(x + 3.0, y + 2.0, 10.0, 9.0), c);
        self.text(
            x + 8.0,
            y + 1.0,
            letter,
            pal::NAVY,
            Size::Small,
            TextAlign::Center,
        );
        self.text(
            x + 16.0,
            y + 1.0,
            label,
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        w
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{Caps, Material, ModelId, ModelParams, SpriteId, SpriteParams, Transform3};

    /// Records rectangles and texts.
    #[derive(Default)]
    pub struct Rec {
        pub rects: std::vec::Vec<(Rect, Color)>,
        pub texts: std::vec::Vec<std::string::String>,
    }

    impl Renderer for Rec {
        fn caps(&self) -> &Caps {
            &Caps::N64
        }
        fn begin_frame(&mut self, _: Color) {}
        fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
        fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
        fn end_frame(&mut self) {}
        fn fill_rect(&mut self, r: Rect, c: Color) {
            self.rects.push((r, c));
        }
        fn draw_text(&mut self, _: FontId, _: Vec2, t: &str, _: &TextParams) {
            self.texts.push(t.into());
        }
        fn measure_text(&self, _: FontId, t: &str, scale: f32) -> Vec2 {
            Vec2::new(t.len() as f32 * 6.0 * scale, 11.0 * scale)
        }
    }

    extern crate std;

    #[test]
    fn the_canvas_scales_by_whole_steps_and_centers() {
        let mut r = Rec::default();
        let ui = Ui::new(&mut r, Vec2::new(320.0, 240.0));
        assert_eq!((ui.scale, ui.origin), (1.0, Vec2::ZERO));
        let mut r = Rec::default();
        let ui = Ui::new(&mut r, Vec2::new(400.0, 240.0));
        assert_eq!((ui.scale, ui.origin), (1.0, Vec2::new(40.0, 0.0)));
        let mut r = Rec::default();
        let ui = Ui::new(&mut r, Vec2::new(1280.0, 720.0));
        assert_eq!(ui.scale, 3.0);
        assert_eq!(ui.px(0.0, 0.0), Vec2::new(160.0, 0.0));
        // A 640x360 PC picture: 1.5x, not a 1x island in the middle.
        let mut r = Rec::default();
        let ui = Ui::new(&mut r, Vec2::new(640.0, 360.0));
        assert_eq!(ui.scale, 1.5);
    }

    #[test]
    fn polygons_fill_with_spans_inside_their_bounds() {
        let mut r = Rec::default();
        let mut ui = Ui::new(&mut r, Vec2::new(320.0, 240.0));
        ui.radar(
            100.0,
            100.0,
            30.0,
            [1.0, 0.5, 0.8, 0.2, 0.9],
            pal::LIME,
            pal::CYAN,
        );
        assert!(r.rects.len() > 20);
        for (rc, _) in &r.rects {
            assert!(rc.pos.x >= 69.0 && rc.pos.x + rc.size.x <= 131.0, "{rc:?}");
            assert!(rc.pos.y >= 69.0 && rc.pos.y <= 131.0, "{rc:?}");
        }
    }

    #[test]
    fn crests_draw_every_kind() {
        use crate::sim::team::Crest;
        for kind in [Crest::Shield, Crest::Round, Crest::Diamond] {
            let mut r = Rec::default();
            let mut ui = Ui::new(&mut r, Vec2::new(320.0, 240.0));
            ui.crest(10.0, 10.0, 24.0, kind, pal::RED, pal::INK, "ROU");
            assert!(r.rects.len() > 10, "{kind:?}");
            assert_eq!(
                r.texts.iter().filter(|t| *t == "ROU").count(),
                2,
                "text and its shadow"
            );
        }
    }
}
