//! The in-match 2D overlay (after the reference's match screen): the
//! scoreboard with both clubs' colors and the clock, the minimap, the
//! controlled player's name and stamina, the kick charge, and event
//! banners (goal, offside, cards, restarts).
//!
//! At `AccessSettings::text_scale` above 100 % (`draw_scaled`, issue #95),
//! the scoreboard, the controlled player's tag and the banners grow up to
//! 200 % while staying inside the 320×240 design canvas and clear of the
//! minimap; at 100 % the layout is pixel-identical to the console goldens.

use core::fmt::Write as _;

use crate::sim::engine::{Match, N, SIDE, side_of};
use crate::sim::pitch::{HALF_LEN, HALF_WID};
use crate::sim::team::{CLUBS, SURNAMES};
use crate::ui::i18n::{Lang, S, tr};
use crate::ui::{Size, Ui, pal};
use trino_core::{Color, Rect, TextAlign, TextBuf};

/// An event banner in flight.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Banner {
    pub text: S,
    /// The side it concerns (its color stripe), if any.
    pub side: Option<u8>,
    /// Seconds left on screen.
    pub left: f32,
}

/// How long a banner stays.
pub const BANNER_SECS: f32 = 2.2;

/// Kit colors each side wears this match (for the scoreboard and map).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct SideColors {
    pub shirt: [Color; 2],
    pub trim: [Color; 2],
}

/// Uppercase a name into a buffer (the UI font has capitals only).
pub fn upper<const N: usize>(s: &str) -> TextBuf<N> {
    let mut b = TextBuf::<N>::new();
    for c in s.chars() {
        let _ = b.write_char(c.to_ascii_uppercase());
    }
    b
}

/// The overlay at 100 % text scale. `db_names[i]` is player `i`'s surname index.
#[allow(clippy::too_many_arguments)]
pub fn draw(
    ui: &mut Ui,
    m: &Match,
    colors: &SideColors,
    active: Option<u8>,
    charge: f32,
    banner: Option<&Banner>,
    lang: Lang,
    names: &[u8; N],
) {
    draw_scaled(ui, m, colors, active, charge, banner, lang, names, 1.0);
}

/// The overlay at `text_scale` (1.0 ..= 2.0, from `AccessSettings::text_factor`).
#[allow(clippy::too_many_arguments)]
pub fn draw_scaled(
    ui: &mut Ui,
    m: &Match,
    colors: &SideColors,
    active: Option<u8>,
    charge: f32,
    banner: Option<&Banner>,
    lang: Lang,
    names: &[u8; N],
    text_scale: f32,
) {
    let k = text_scale.clamp(1.0, 2.0);
    scoreboard(ui, m, colors, k);
    minimap(ui, m, colors);
    if let Some(a) = active {
        player_tag(ui, m, a as usize, names[a as usize], charge, k);
    }
    if let Some(b) = banner {
        draw_banner(ui, b, colors, lang, k);
    }
}

fn scoreboard(ui: &mut Ui, m: &Match, c: &SideColors, k: f32) {
    let (x, y) = (6.0, 6.0);
    let wc = 34.0 * k;
    let ws = 34.0 * k;
    let w_clk = 38.0 * k;
    let h = 13.0 * k;
    let trim_h = 2.0 * k;
    let ty = y + k;
    // Home: color block with the club's short name.
    ui.slant(Rect::new(x, y, wc, h), c.shirt[0], 0.0);
    ui.fill(Rect::new(x, y + h - trim_h, wc, trim_h), c.trim[0]);
    ui.text_shadow_scaled(
        x + wc * 0.5,
        ty,
        CLUBS[m.clubs[0] as usize].short,
        pal::INK,
        Size::Small,
        k,
        TextAlign::Center,
    );
    // Score.
    let xs = x + wc;
    ui.fill(Rect::new(xs, y, ws, h), Color::rgba(14, 16, 30, 235));
    let mut b = TextBuf::<12>::new();
    let _ = write!(b, "{} - {}", m.score[0], m.score[1]);
    ui.text_scaled(
        xs + ws * 0.5,
        ty,
        b.as_str(),
        pal::INK,
        Size::Small,
        k,
        TextAlign::Center,
    );
    // Away.
    let xa = xs + ws;
    ui.slant(Rect::new(xa, y, wc, h), c.shirt[1], 0.0);
    ui.fill(Rect::new(xa, y + h - trim_h, wc, trim_h), c.trim[1]);
    ui.text_shadow_scaled(
        xa + wc * 0.5,
        ty,
        CLUBS[m.clubs[1] as usize].short,
        pal::INK,
        Size::Small,
        k,
        TextAlign::Center,
    );
    // Clock, with a cyan edge like the reference.
    let xk = xa + wc + 2.0 * k;
    ui.fill(Rect::new(xk, y, w_clk, h), Color::rgba(14, 16, 30, 235));
    ui.fill(Rect::new(xk, y, 2.0 * k, h), pal::CYAN);
    b.clear();
    let _ = write!(b, "{:02}:{:02}", m.minute().min(99), m.second());
    ui.text_scaled(
        xk + w_clk * 0.5 + k,
        ty,
        b.as_str(),
        pal::INK,
        Size::Small,
        k,
        TextAlign::Center,
    );
}

fn minimap(ui: &mut Ui, m: &Match, c: &SideColors) {
    let (w, h) = (76.0, 50.0);
    let x = 160.0 - w * 0.5;
    let y = 240.0 - h - 6.0;
    let r = Rect::new(x, y, w, h);
    ui.fill(r, Color::rgba(30, 90, 40, 190));
    ui.border(r, Color::rgba(230, 240, 230, 200), 1.0);
    ui.fill(
        Rect::new(x + w * 0.5, y, 1.0, h),
        Color::rgba(230, 240, 230, 160),
    );
    let bw = w * 16.5 / 105.0;
    let bh = h * 40.3 / 68.0;
    ui.border(
        Rect::new(x, y + (h - bh) * 0.5, bw, bh),
        Color::rgba(230, 240, 230, 160),
        1.0,
    );
    ui.border(
        Rect::new(x + w - bw, y + (h - bh) * 0.5, bw, bh),
        Color::rgba(230, 240, 230, 160),
        1.0,
    );
    let map = |px: f32, pz: f32| {
        (
            x + (px / HALF_LEN.to_f32() * 0.5 + 0.5) * w,
            y + (pz / HALF_WID.to_f32() * 0.5 + 0.5) * h,
        )
    };
    for i in 0..N {
        let p = &m.players[i];
        if p.off {
            continue;
        }
        let (mx, my) = map(p.pos.x.to_f32(), p.pos.y.to_f32());
        // One rectangle per dot (every 2D draw costs ~25 µs on the N64).
        let col = c.shirt[side_of(i)];
        ui.fill(Rect::new(mx - 1.0, my - 1.0, 2.0, 2.0), col);
    }
    let (bx, by) = map(m.ball.pos.x.to_f32(), m.ball.pos.z.to_f32());
    ui.fill(Rect::new(bx - 1.0, by - 1.0, 2.0, 2.0), Color::WHITE);
    let _ = SIDE;
}

fn player_tag(ui: &mut Ui, m: &Match, i: usize, surname: u8, charge: f32, k: f32) {
    let p = &m.players[i];
    let mut b = TextBuf::<24>::new();
    let _ = write!(b, "{} ", p.number);
    let name: TextBuf<16> = upper(SURNAMES[surname as usize % SURNAMES.len()]);
    let _ = b.write_str(name.as_str());
    let tw = ui.measure_scaled(b.as_str(), Size::Small, k);
    let w = if k <= 1.01 {
        92.0
    } else {
        (tw + 14.0).clamp(92.0, 308.0)
    };
    let h = if k <= 1.01 { 24.0 } else { 13.0 + 11.0 * k };
    let x = 6.0;
    // Stay clear of the minimap (x = 122..198, y = 184..234) when a scaled
    // tag grows wider than the bottom-left corner.
    let y = if x + w > 118.0 {
        184.0 - h - 2.0
    } else {
        240.0 - h - 6.0
    };
    ui.slant(Rect::new(x, y, w, h), Color::rgba(14, 16, 34, 220), 4.0);
    ui.fill(Rect::new(x, y, 3.0, h), pal::CYAN);
    ui.text_scaled(
        x + 7.0,
        y + 2.0,
        b.as_str(),
        pal::INK,
        Size::Small,
        k,
        TextAlign::Left,
    );
    let bar_y = y + h - 9.0;
    // Stamina.
    ui.bar(
        Rect::new(x + 7.0, bar_y, 60.0, 3.0),
        p.stamina.to_f32(),
        pal::LIME,
    );
    if charge > 0.0 {
        // The kick's power: green to red.
        let col = if charge < 0.5 {
            pal::LIME
        } else if charge < 0.85 {
            pal::YELLOW
        } else {
            pal::RED
        };
        ui.bar(Rect::new(x + 7.0, bar_y + 4.0, 80.0, 3.0), charge, col);
    }
}

fn draw_banner(ui: &mut Ui, b: &Banner, c: &SideColors, lang: Lang, k: f32) {
    let t = tr(lang, b.text);
    let big = matches!(
        b.text,
        S::Goal | S::HalfTime | S::FullTime | S::Penalty | S::RedCard
    );
    let size = if big { Size::Big } else { Size::Small };
    let tk = if big { 1.0 } else { k };
    let tw = ui.measure_scaled(t, size, tk);
    let w = (tw + 36.0).clamp(100.0, 304.0);
    let h = if big { 30.0 } else { 7.0 + 11.0 * tk };
    let x = 160.0 - w * 0.5;
    let y = 64.0;
    // Slide in over the first quarter second.
    let anim = ((BANNER_SECS - b.left) / 0.25).min(1.0);
    let x = x - (1.0 - anim) * 80.0;
    let stripe = match b.side {
        Some(s) => c.shirt[s as usize],
        None => pal::CYAN,
    };
    let fill = match b.text {
        S::YellowCard => Color::rgba(90, 70, 0, 235),
        S::RedCard => Color::rgba(110, 10, 20, 235),
        _ => Color::rgba(14, 16, 40, 235),
    };
    ui.slant(Rect::new(x, y, w, h), fill, 8.0);
    ui.slant(Rect::new(x - 6.0, y, 8.0, h), stripe, 8.0);
    ui.slant(Rect::new(x + w - 2.0, y, 8.0, h), stripe, 8.0);
    let col = if matches!(b.text, S::Goal) {
        pal::YELLOW
    } else {
        pal::INK
    };
    ui.text_shadow_scaled(
        160.0 - (1.0 - anim) * 80.0 + 3.0,
        y + (h - if big { 22.0 } else { 11.0 * tk }) * 0.5,
        t,
        col,
        size,
        tk,
        TextAlign::Center,
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::{
        Caps, FontId, Material, ModelId, ModelParams, Renderer, SpriteId, SpriteParams, TextParams,
        Transform3, Vec2,
    };

    struct HudProbe {
        texts: Vec<(Rect, String)>,
        fills: Vec<Rect>,
    }

    impl Renderer for HudProbe {
        fn caps(&self) -> &Caps {
            &Caps::N64
        }
        fn begin_frame(&mut self, _: Color) {}
        fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
        fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
        fn end_frame(&mut self) {}
        fn fill_rect(&mut self, r: Rect, _: Color) {
            self.fills.push(r);
        }
        fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
            let cell = if font == crate::ui::F_BIG {
                Vec2::new(10.0, 22.0)
            } else if font == crate::ui::F_HUGE {
                Vec2::new(20.0, 44.0)
            } else {
                Vec2::new(5.0, 11.0)
            };
            Vec2::new(text.chars().count() as f32 * cell.x * scale, cell.y * scale)
        }
        fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, p: &TextParams) {
            let sz = self.measure_text(font, text, p.scale);
            let x = match p.align {
                TextAlign::Left => pos.x,
                TextAlign::Center => pos.x - sz.x * 0.5,
                TextAlign::Right => pos.x - sz.x,
            };
            self.texts
                .push((Rect::new(x, pos.y, sz.x, sz.y), text.into()));
        }
    }

    #[test]
    fn hud_fits_at_100_and_200_percent_text_in_both_languages() {
        let (setup, names, _) = crate::quick_setup(0, 1, 1, 180, [true, false]);
        let m = Match::new(&setup);
        let colors = SideColors {
            shirt: [pal::RED, pal::BLUE],
            trim: [Color::WHITE, Color::WHITE],
        };
        let banners = [
            S::Goal,
            S::HalfTime,
            S::FullTime,
            S::Offside,
            S::YellowCard,
            S::RedCard,
            S::Penalty,
            S::Corner,
            S::ThrowIn,
            S::GoalKick,
            S::FreeKick,
            S::Save,
            S::Post,
        ];
        for lang in Lang::ALL {
            for k in [1.0f32, 2.0] {
                for &b_txt in &banners {
                    let banner = Banner {
                        text: b_txt,
                        side: Some(0),
                        left: 1.0,
                    };
                    let mut probe = HudProbe {
                        texts: Vec::new(),
                        fills: Vec::new(),
                    };
                    let mut ui = Ui::new(&mut probe, Vec2::new(320.0, 240.0));
                    draw_scaled(
                        &mut ui,
                        &m,
                        &colors,
                        Some(9),
                        0.75,
                        Some(&banner),
                        lang,
                        &names,
                        k,
                    );
                    for (r, txt) in &probe.texts {
                        assert!(
                            r.pos.x >= 0.0
                                && r.pos.y >= 0.0
                                && r.pos.x + r.size.x <= 320.0
                                && r.pos.y + r.size.y <= 240.0,
                            "{txt:?} at {r:?} off 320x240 at scale {k} ({lang:?})"
                        );
                    }
                }
            }
        }
    }
}
