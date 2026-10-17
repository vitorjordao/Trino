//! The in-match 2D overlay (after the reference's match screen): the
//! scoreboard with both clubs' colors and the clock, the minimap, the
//! controlled player's name and stamina, the kick charge, and event
//! banners (goal, offside, cards, restarts).

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

/// The overlay. `db_names[i]` is player `i`'s surname index.
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
    scoreboard(ui, m, colors);
    minimap(ui, m, colors);
    if let Some(a) = active {
        player_tag(ui, m, a as usize, names[a as usize], charge);
    }
    if let Some(b) = banner {
        draw_banner(ui, b, colors, lang);
    }
}

fn scoreboard(ui: &mut Ui, m: &Match, c: &SideColors) {
    let (x, y) = (6.0, 6.0);
    // Home: color block with the club's short name.
    ui.slant(Rect::new(x, y, 34.0, 13.0), c.shirt[0], 0.0);
    ui.fill(Rect::new(x, y + 11.0, 34.0, 2.0), c.trim[0]);
    ui.text_shadow(
        x + 17.0,
        y + 1.0,
        CLUBS[m.clubs[0] as usize].short,
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    // Score.
    ui.fill(
        Rect::new(x + 34.0, y, 34.0, 13.0),
        Color::rgba(14, 16, 30, 235),
    );
    let mut b = TextBuf::<12>::new();
    let _ = write!(b, "{} - {}", m.score[0], m.score[1]);
    ui.text(
        x + 51.0,
        y + 1.0,
        b.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    // Away.
    ui.slant(Rect::new(x + 68.0, y, 34.0, 13.0), c.shirt[1], 0.0);
    ui.fill(Rect::new(x + 68.0, y + 11.0, 34.0, 2.0), c.trim[1]);
    ui.text_shadow(
        x + 85.0,
        y + 1.0,
        CLUBS[m.clubs[1] as usize].short,
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    // Clock, with a cyan edge like the reference.
    ui.fill(
        Rect::new(x + 104.0, y, 38.0, 13.0),
        Color::rgba(14, 16, 30, 235),
    );
    ui.fill(Rect::new(x + 104.0, y, 2.0, 13.0), pal::CYAN);
    b.clear();
    let _ = write!(b, "{:02}:{:02}", m.minute().min(99), m.second());
    ui.text(
        x + 124.0,
        y + 1.0,
        b.as_str(),
        pal::INK,
        Size::Small,
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

fn player_tag(ui: &mut Ui, m: &Match, i: usize, surname: u8, charge: f32) {
    let p = &m.players[i];
    let (x, y) = (6.0, 240.0 - 30.0);
    ui.slant(
        Rect::new(x, y, 92.0, 24.0),
        Color::rgba(14, 16, 34, 220),
        4.0,
    );
    ui.fill(Rect::new(x, y, 3.0, 24.0), pal::CYAN);
    let mut b = TextBuf::<24>::new();
    let _ = write!(b, "{} ", p.number);
    let name: TextBuf<16> = upper(SURNAMES[surname as usize % SURNAMES.len()]);
    let _ = b.write_str(name.as_str());
    ui.text(
        x + 7.0,
        y + 2.0,
        b.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    // Stamina.
    ui.bar(
        Rect::new(x + 7.0, y + 15.0, 60.0, 3.0),
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
        ui.bar(Rect::new(x + 7.0, y + 19.0, 80.0, 3.0), charge, col);
    }
}

fn draw_banner(ui: &mut Ui, b: &Banner, c: &SideColors, lang: Lang) {
    let t = tr(lang, b.text);
    let big = matches!(
        b.text,
        S::Goal | S::HalfTime | S::FullTime | S::Penalty | S::RedCard
    );
    let size = if big { Size::Big } else { Size::Small };
    let tw = ui.measure(t, size);
    let w = (tw + 36.0).max(100.0);
    let h = if big { 30.0 } else { 18.0 };
    let x = 160.0 - w * 0.5;
    let y = 64.0;
    // Slide in over the first quarter second.
    let k = ((BANNER_SECS - b.left) / 0.25).min(1.0);
    let x = x - (1.0 - k) * 80.0;
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
    ui.text_shadow(
        160.0 - (1.0 - k) * 80.0 + 3.0,
        y + (h - if big { 22.0 } else { 11.0 }) * 0.5,
        t,
        col,
        size,
        TextAlign::Center,
    );
}
