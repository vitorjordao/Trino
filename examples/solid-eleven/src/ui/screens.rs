//! Every menu screen, drawn after the reference art (`/home/v/project
//! futebol`): the title with its menu column and profile card, quick match,
//! the club pick, club management, the table, team selection, the player
//! editor with its point budget, the transfer market, settings, the match
//! report, pause, and the 3DS's touch screen.

use core::fmt::Write as _;

use crate::game::app::{EDIT_ROWS as EDIT_ROW_COUNT, QUICK_GO, Scene, SettingRow, SolidElevenGame};
use crate::game::session::dress;
use crate::render::anim::AnimState;
use crate::render::clips_gen::ClipName;
use crate::render::hud::upper;
use crate::render::{self, Hair, HairStyle, Look, SKIN_TONES, rgb};
use crate::sim::engine::{N, SIDE, side_of};
use crate::sim::league::ROUNDS;
use crate::sim::team::{
    self, ATTRS, Appearance, CLUBS, Crest, FORMATIONS, PlayerRec, SURNAMES, TEAMS,
};
use crate::ui::i18n::{Lang, S, pos_short, tr, tr_fmt};
use crate::ui::{Size, Ui, pal};
use trino_core::i18n::Arg;
use trino_core::ui::NO_ITEM;
use trino_core::{Color, Rect, Renderer, Screen, TextAlign, TextBuf, Vec2};

type Buf = TextBuf<48>;

fn buf() -> Buf {
    TextBuf::new()
}

/// Thousands with a dot, as the reference prints coins (15.200).
fn coins(v: i32) -> TextBuf<16> {
    let mut b = TextBuf::<16>::new();
    if v < 0 {
        let _ = b.write_char('-');
    }
    let v = v.unsigned_abs();
    if v >= 1000 {
        let _ = write!(b, "{}.{:03}", v / 1000, v % 1000);
    } else {
        let _ = write!(b, "{v}");
    }
    b
}

/// A player's name line: "10. SILVA (ATA)".
fn name_line(p: &PlayerRec, lang: Lang) -> Buf {
    let mut b = buf();
    let name: TextBuf<16> = upper(p.name());
    let _ = write!(
        b,
        "{}. {} ({})",
        p.number,
        name.as_str(),
        pos_short(lang, p.pos)
    );
    b
}

/// The look of player `i` in his club's home kit.
fn look_of(g: &SolidElevenGame, i: usize) -> Look {
    let p = &g.world.players[i];
    let c = &CLUBS[g.world.club_of[i] as usize % TEAMS];
    let keeper = p.pos == team::Pos::Gk;
    Look::of(
        if keeper { c.gk } else { c.home.shirt },
        if keeper {
            team::GK_SHORTS
        } else {
            c.home.shorts
        },
        if keeper { c.gk } else { c.home.socks },
        SKIN_TONES[p.skin as usize % 8],
    )
    .with_hair(Hair::of(p.style, p.hair))
}

/// A tiny portrait: skin, hair, shirt (list rows).
fn portrait(ui: &mut Ui, x: f32, y: f32, look: &Look) {
    ui.fill(Rect::new(x, y, 16.0, 16.0), Color::rgb(40, 52, 110));
    ui.fill(Rect::new(x + 2.0, y + 11.0, 12.0, 5.0), look.shirt);
    ui.fill(Rect::new(x + 5.0, y + 3.0, 6.0, 8.0), look.skin);
    if look.hair.style != HairStyle::Bald {
        ui.fill(Rect::new(x + 5.0, y + 2.0, 6.0, 2.0), look.hair.color);
    }
}

fn crest_of(ui: &mut Ui, x: f32, y: f32, w: f32, c: u8, label: bool) {
    let club = &CLUBS[c as usize % TEAMS];
    ui.crest(
        x,
        y,
        w,
        club.crest,
        rgb(club.home.shirt),
        rgb(club.home.trim),
        if label { club.short } else { "" },
    );
}

use crate::game::menu::Hits;

pub const HIT_BACK: u8 = 254;

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum Btn {
    A,
    B,
    X,
    Y,
}

pub(crate) fn btn_prompt(pad: bool, b: Btn) -> (&'static str, Color) {
    if pad {
        match b {
            Btn::A => ("A", pal::GREEN),
            Btn::B => ("B", pal::RED),
            Btn::X => ("X", pal::BLUE),
            Btn::Y => ("Y", pal::YELLOW),
        }
    } else {
        match b {
            Btn::A => ("ENTER", pal::GREEN),
            Btn::B => ("ESC", pal::RED),
            Btn::X => ("C", pal::BLUE),
            Btn::Y => ("V", pal::YELLOW),
        }
    }
}

/// The slanted screen header ("SELEÇÃO DO TIME").
fn header(ui: &mut Ui, hits: &mut Hits, title: &str, back: bool) {
    ui.slant(Rect::new(-4.0, 6.0, 200.0, 22.0), pal::NAVY, 6.0);
    if back {
        let r = Rect::new(-4.0, 6.0, 24.0, 22.0);
        ui.slant(r, pal::CYAN, 6.0);
        ui.text(8.0, 10.0, "<", pal::NAVY, Size::Small, TextAlign::Center);
        let hit = ui.rect(Rect::new(0.0, 4.0, 26.0, 26.0));
        hits.add(hit, Screen::Top, HIT_BACK);
    }
    ui.text_shadow(24.0, 9.0, title, pal::INK, Size::Small, TextAlign::Left);
}

/// The footer of button hints, after the reference.
fn footer(ui: &mut Ui, hits: &mut Hits, pad: bool, lang: Lang, hints: &[(Btn, S)]) {
    let mut x = 6.0;
    for (btn, s) in hints {
        let (letter, c) = btn_prompt(pad, *btn);
        let w = ui.hint(x, 224.0, letter, c, tr(lang, *s));
        if *btn == Btn::B {
            let hit = ui.rect(Rect::new(x, 222.0, w, 18.0));
            hits.add(hit, Screen::Top, HIT_BACK);
        }
        x += w + 4.0;
    }
}

/// Draw the current screen (top) and the 3DS's touch screen.
pub fn draw(g: &mut SolidElevenGame, r: &mut dyn Renderer) {
    g.hits.clear();
    let screen = g.screen;
    let aspect = screen.x / screen.y.max(1.0);
    let lang = g.settings.lang;
    match g.scene {
        Scene::Match | Scene::Pause | Scene::HalfTime | Scene::Substitutions { .. } => {
            if let Some(s) = &g.session {
                s.render(r, screen, lang);
            }
            match g.scene {
                Scene::Pause => pause(g, r),
                Scene::HalfTime => half_time(g, r, screen),
                Scene::Substitutions { from_half_time } => {
                    substitutions(g, r, screen, from_half_time)
                }
                _ => {}
            }
            captions(g, r);
        }
        Scene::Title => {
            title_backdrop(g, r, aspect);
            title(g, r, screen);
        }
        Scene::Trophy => trophy(g, r, screen, aspect),
        Scene::Quick => {
            title_backdrop(g, r, aspect);
            quick(g, r, screen);
        }
        Scene::PickClub { .. } => {
            title_backdrop(g, r, aspect);
            pick_club(g, r, screen);
        }
        Scene::Club => {
            title_backdrop(g, r, aspect);
            club(g, r, screen);
        }
        Scene::Table => {
            title_backdrop(g, r, aspect);
            table(g, r, screen);
        }
        Scene::Teams => {
            let (list, n) = g.squad_list(g.cur.team_club);
            if n > 0 {
                let i = list[g.cur.squad.min(n - 1)] as usize;
                let anim = idle_anim(g.time);
                render::menu::pedestal(
                    r,
                    &look_of(g, i),
                    &anim,
                    g.spin,
                    pal::CYAN,
                    aspect,
                    0.0,
                    g.settings.graphics == 1,
                );
            }
            teams(g, r, screen);
        }
        Scene::Edit => {
            if let Some((i, _)) = g.edit {
                let anim = idle_anim(g.time);
                render::menu::pedestal(
                    r,
                    &look_of(g, i),
                    &anim,
                    g.spin,
                    pal::CYAN,
                    aspect,
                    0.0,
                    g.settings.graphics == 1,
                );
            }
            edit(g, r, screen);
        }
        Scene::Market => {
            let (list, n) = g.market_list();
            if n > 0 {
                let i = list[g.cur.market.min(n - 1)] as usize;
                let anim = idle_anim(g.time);
                render::menu::pedestal(
                    r,
                    &look_of(g, i),
                    &anim,
                    g.spin,
                    pal::LIME,
                    aspect,
                    // In the gap between the list and the card.
                    0.6,
                    g.settings.graphics == 1,
                );
            }
            market(g, r, screen);
        }
        Scene::Settings | Scene::Confirm => {
            title_backdrop(g, r, aspect);
            settings(g, r, screen);
        }
        Scene::Controls => {
            title_backdrop(g, r, aspect);
            let lang = g.settings.lang;
            let pad = g.pad_layout();
            let mut ui = Ui::new(r, screen);
            ui.fill(
                Rect::new(-200.0, 0.0, 720.0, 240.0),
                Color::rgba(6, 8, 20, 200),
            );
            header(&mut ui, &mut g.hits, tr(lang, S::Controls), true);
            crate::ui::controls::draw(g, &mut ui);
            footer(
                &mut ui,
                &mut g.hits,
                pad,
                lang,
                &[(Btn::A, S::Change), (Btn::B, S::Back)],
            );
        }
        Scene::Results => {
            title_backdrop(g, r, aspect);
            results(g, r, screen);
        }
    }
    if let Some(t) = g.toast {
        toast(g, r, t, lang);
    }
    if r.on_screen(Screen::Bottom) {
        bottom(g, r);
    }
}

/// The toast: a UI screen over the menu, at the player's text scale.
fn toast(g: &mut SolidElevenGame, r: &mut dyn Renderer, t: crate::game::app::Toast, lang: Lang) {
    use super::scr::{flag, txt};
    struct Toast(crate::game::app::Toast, Lang);
    impl trino_core::ui::UiData for Toast {
        fn value(&self, key: &str, _: u16) -> Option<trino_core::ui::UiValue> {
            match key {
                "good" => flag(self.0.good),
                "bad" => flag(!self.0.good),
                "text" => txt(tr(self.1, self.0.text)),
                _ => None,
            }
        }
    }
    let data = Toast(t, lang);
    let env = trino_core::UiEnv::of(r)
        .with_access(&g.access)
        .with_data(&data);
    g.toast_ui.draw(r, &super::toast_gen::SCREEN, &env);
}

fn idle_anim(t: f32) -> AnimState {
    let mut a = AnimState::new(ClipName::Idle);
    a.time = t;
    a
}

/// How the ceremony's players look: the five shown (9, 7, 10, 4, 2) each
/// with a hair style of his own.
const CEREMONY_PEOPLE: [Appearance; 22] = {
    // (skin, hair color, style) per player.
    const L: [(u8, u8, u8); 22] = [
        (1, 0, 0),
        (3, 1, 0),
        (5, 0, 5),
        (5, 0, 1),
        (0, 4, 4),
        (6, 0, 2),
        (4, 1, 0),
        (7, 0, 2),
        (2, 2, 0),
        (1, 1, 0),
        (2, 3, 3),
        (2, 0, 0),
        (5, 0, 1),
        (0, 3, 0),
        (6, 0, 2),
        (4, 1, 5),
        (7, 0, 1),
        (2, 2, 0),
        (1, 3, 3),
        (3, 1, 0),
        (2, 0, 4),
        (5, 0, 2),
    ];
    let mut out = [Appearance {
        skin: 0,
        hair: 0,
        style: 0,
    }; 22];
    let mut i = 0;
    while i < 22 {
        out[i] = Appearance {
            skin: L[i].0,
            hair: L[i].1,
            style: L[i].2,
        };
        i += 1;
    }
    out
};

/// The match's captions (issue #60), over everything, in the menu font and
/// the player's language; nothing while the queue is empty.
fn captions(g: &SolidElevenGame, r: &mut dyn Renderer) {
    let style = trino_core::CaptionStyle {
        font: trino_core::FontId::from_path("fonts/se_ui"),
        ..trino_core::CaptionStyle::DEFAULT
    };
    let lang = match g.settings.lang {
        super::i18n::Lang::En => 0,
        super::i18n::Lang::Pt => 1,
    };
    match g.caption_texts[lang].table() {
        Some(t) => g.captions.draw(r, t, &style),
        None => g.captions.draw(r, &trino_core::ui::NoStrings, &style),
    }
}

/// Five players of `club` in its home kit: the captain first.
fn ceremony_looks(club: u8) -> [Look; 5] {
    let other = if club == 0 { 1 } else { 0 };
    let d = dress([club, other], &CEREMONY_PEOPLE);
    [d.looks[9], d.looks[7], d.looks[10], d.looks[4], d.looks[2]]
}

fn title_backdrop(g: &SolidElevenGame, r: &mut dyn Renderer, aspect: f32) {
    // The club the menus are about lifts the trophy.
    let club = if g.career_on {
        g.world.user
    } else {
        g.quick.home
    };
    render::menu::title(
        r,
        &ceremony_looks(club),
        &g.menu_anims,
        aspect,
        g.time,
        g.settings.graphics == 1,
    );
}

// ----------------------------------------------------------------- trophy

/// The season's end: the champion's captain lifts the trophy; the banner
/// names the champion and how the user's club finished.
fn trophy(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2, aspect: f32) {
    let lang = g.settings.lang;
    let order = g.world.standings();
    let champ = order[0];
    // Off the halfway line: seen along it, the line ran up behind the
    // captain as if out of his shorts.
    let at = trino_core::Vec3::new(5.0, 0.0, 0.0);
    let cam = render::CamState {
        focus: at + trino_core::Vec3::new(0.0, 1.3, 1.0),
        height: 0.6,
        back: 7.5,
        fov: 0.72,
        wide: false,
    };
    render::menu::ceremony(
        r,
        &ceremony_looks(champ),
        &g.menu_anims,
        aspect,
        &cam,
        at,
        g.time,
        g.settings.graphics == 1,
    );
    let mut ui = Ui::new(r, screen);
    let club = &CLUBS[champ as usize];
    ui.slant(Rect::new(40.0, 10.0, 240.0, 46.0), pal::NAVY, 6.0);
    ui.slant_border(Rect::new(40.0, 10.0, 240.0, 46.0), pal::YELLOW, 6.0, 1.0);
    crest_of(&mut ui, 50.0, 15.0, 28.0, champ, false);
    ui.text_shadow(
        176.0,
        14.0,
        tr(lang, S::Champion),
        pal::YELLOW,
        Size::Big,
        TextAlign::Center,
    );
    let name: TextBuf<20> = upper(club.name);
    let mut b = buf();
    let _ = write!(
        b,
        "{}  {} {}",
        name.as_str(),
        tr(lang, S::Season),
        g.world.year
    );
    ui.text(
        176.0,
        40.0,
        b.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    let user = g.world.user;
    let place = order.iter().position(|&c| c == user).unwrap_or(0) + 1;
    b.clear();
    if place == 1 {
        let _ = write!(b, "{}", tr(lang, S::LeagueWon));
    } else {
        let _ = write!(b, "{} {}/{}", tr(lang, S::FinishedAt), place, order.len());
    }
    let w = ui.measure(b.as_str(), Size::Small) + 24.0;
    ui.slant(
        Rect::new(160.0 - w * 0.5, 176.0, w, 16.0),
        Color::rgba(10, 12, 30, 235),
        4.0,
    );
    let c = if place == 1 { pal::LIME } else { pal::CYAN };
    ui.slant_border(Rect::new(160.0 - w * 0.5, 176.0, w, 16.0), c, 4.0, 1.0);
    ui.text(160.0, 178.0, b.as_str(), c, Size::Small, TextAlign::Center);
    let pad = g.pad_layout();
    footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Continue)]);
}

// ------------------------------------------------------------------ title

fn title(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let mut ui = Ui::new(r, screen);
    // Logo: a shield with the name over it.
    ui.polygon(
        &[
            Vec2::new(10.0, 8.0),
            Vec2::new(78.0, 8.0),
            Vec2::new(78.0, 40.0),
            Vec2::new(44.0, 62.0),
            Vec2::new(10.0, 40.0),
        ],
        pal::RED,
    );
    ui.polygon(
        &[
            Vec2::new(13.0, 11.0),
            Vec2::new(75.0, 11.0),
            Vec2::new(75.0, 39.0),
            Vec2::new(44.0, 58.0),
            Vec2::new(13.0, 39.0),
        ],
        Color::rgb(30, 70, 190),
    );
    // The ball over the shield's corner.
    ui.polygon(
        &[
            Vec2::new(74.0, 6.0),
            Vec2::new(82.0, 9.0),
            Vec2::new(85.0, 17.0),
            Vec2::new(82.0, 25.0),
            Vec2::new(74.0, 28.0),
            Vec2::new(66.0, 25.0),
            Vec2::new(63.0, 17.0),
            Vec2::new(66.0, 9.0),
        ],
        Color::rgb(245, 245, 250),
    );
    ui.fill(Rect::new(71.0, 14.0, 6.0, 6.0), Color::rgb(30, 30, 40));
    ui.text(
        18.0 + 1.5,
        13.0 + 1.5,
        "SOLID",
        Color::rgb(20, 30, 90),
        Size::Big,
        TextAlign::Left,
    );
    ui.text(18.0, 13.0, "SOLID", pal::INK, Size::Big, TextAlign::Left);
    ui.text(
        16.0 + 1.5,
        37.0 + 1.5,
        "ELEVEN",
        Color::rgb(90, 10, 30),
        Size::Big,
        TextAlign::Left,
    );
    ui.text(16.0, 37.0, "ELEVEN", pal::INK, Size::Big, TextAlign::Left);
    ui.text_shadow(
        88.0,
        50.0,
        "LOW POLY FOOTBALL",
        pal::DIM,
        Size::Small,
        TextAlign::Left,
    );

    let items = [
        (S::PlayNow, S::QuickMatch, pal::CYAN),
        (S::Career, S::ManageClub, pal::RED),
        (S::Customize, S::CustomizeSub, pal::YELLOW),
        (S::Transfers, S::TransfersSub, pal::GREEN),
        (S::Settings, S::SettingsSub, pal::BLUE),
        (S::Exit, S::ExitSub, pal::PINK),
    ];
    for (k, (main, sub, c)) in items.iter().enumerate() {
        let y = 68.0 + k as f32 * 25.0;
        let sel = g.cur.title == k;
        let w = if sel { 160.0 } else { 150.0 };
        let r = Rect::new(10.0, y, w, 22.0);
        ui.slant(
            r,
            if sel {
                Color::rgba(20, 40, 90, 245)
            } else {
                pal::PANEL
            },
            5.0,
        );
        ui.slant(Rect::new(10.0, y, 3.0, 22.0), *c, 5.0);
        if sel {
            ui.slant_border(r, *c, 5.0, 1.0);
        }
        ui.text(
            18.0,
            y + 1.0,
            tr(lang, *main),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        ui.text(
            18.0,
            y + 11.0,
            tr(lang, *sub),
            *c,
            Size::Small,
            TextAlign::Left,
        );
        let hit = ui.rect(r);
        g.hits.add(hit, Screen::Top, k as u8);
    }
    // Profile card (the reference's bottom right).
    let (x, y) = (204.0, 150.0);
    ui.panel(Rect::new(x, y, 110.0, 64.0), pal::CYAN);
    ui.fill(Rect::new(x, y, 110.0, 12.0), Color::rgba(30, 60, 120, 240));
    ui.text(
        x + 55.0,
        y + 1.0,
        if g.career_on {
            tr(lang, S::Club)
        } else {
            tr(lang, S::QuickMatch)
        },
        pal::CYAN,
        Size::Small,
        TextAlign::Center,
    );
    let c = if g.career_on {
        g.world.user
    } else {
        g.quick.home
    };
    crest_of(&mut ui, x + 6.0, y + 16.0, 22.0, c, false);
    // The name on two lines when it is too long for one (VIOLET WANDERERS).
    let name: TextBuf<20> = upper(CLUBS[c as usize].name);
    let (first, second) = ui.wrap2(name.as_str(), Size::Small, 110.0 - 33.0 - 3.0);
    ui.text(
        x + 33.0,
        y + 16.0,
        first,
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let below = if second.is_empty() {
        0.0
    } else {
        ui.text(
            x + 33.0,
            y + 26.0,
            second,
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        10.0
    };
    let mut b = buf();
    let _ = write!(
        b,
        "{} {}",
        coins(g.world.clubs[c as usize].budget).as_str(),
        tr(lang, S::Coins)
    );
    ui.text(
        x + 33.0,
        y + 27.0 + below,
        b.as_str(),
        pal::YELLOW,
        Size::Small,
        TextAlign::Left,
    );
    if g.career_on {
        b.clear();
        let _ = write!(
            b,
            "{} {}/{}  {}",
            tr(lang, S::Round),
            (g.world.round as usize + 1).min(ROUNDS),
            ROUNDS,
            g.world.year
        );
        ui.text(
            x + 6.0,
            y + 48.0,
            b.as_str(),
            pal::DIM,
            Size::Small,
            TextAlign::Left,
        );
    }
    let pad = g.pad_layout();
    footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Select)]);
}

// ------------------------------------------------------------ quick match

fn quick(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, int, txt};
    let screen = &super::quick_gen::SCREEN;
    let big = g.access.clamped().text_factor() >= 1.5;
    let focus = match g.cur.quick {
        0 => (if big { "card_home_big" } else { "card_home" }, NO_ITEM),
        1 => (if big { "card_away_big" } else { "card_away" }, NO_ITEM),
        QUICK_GO => ("go", NO_ITEM),
        k => ("row", (k - 2) as u16),
    };
    super::scr::show(
        g,
        r,
        screen,
        Some(focus),
        super::scr::rev(&(g.quick, g.settings.difficulty, g.world.year)),
        |g, key, item| {
            let lang = g.settings.lang;
            let club = |c: u8| &CLUBS[c as usize];
            let ovr = |c: u8| {
                let mut b = buf();
                let _ = write!(
                    b,
                    "{} {}   {}",
                    tr(lang, S::Ovr),
                    g.world.strength(c),
                    FORMATIONS[g.world.clubs[c as usize].formation as usize % FORMATIONS.len()]
                        .name
                );
                b
            };
            match key {
                "rows" => return int(5),
                "home_name" => return txt(upper::<20>(club(g.quick.home).name).as_str()),
                "away_name" => return txt(upper::<20>(club(g.quick.away).name).as_str()),
                "home_ovr" => return txt(ovr(g.quick.home).as_str()),
                "away_ovr" => return txt(ovr(g.quick.away).as_str()),
                _ => {}
            }
            let side = match g.quick.side {
                0 => S::SideHome,
                1 => S::SideAway,
                _ => S::SideCpu,
            };
            let sky = [S::Day, S::Dusk, S::Night][g.quick.sky as usize % 3];
            let stadium =
                [S::StadiumArena, S::StadiumHarbour, S::StadiumPark][g.quick.stadium as usize % 3];
            let (label, value) = match item {
                0 => (S::Team, side),
                1 => (S::Stadium, stadium),
                2 => (S::KickoffTime, sky),
                3 => (S::Weather, if g.quick.rain { S::Rain } else { S::Clear }),
                4 => (
                    S::Difficulty,
                    [S::Easy, S::Normal, S::Hard][g.settings.difficulty as usize % 3],
                ),
                _ => return None,
            };
            match key {
                "label" => txt(tr(lang, label)),
                "value" => {
                    let mut b = buf();
                    let _ = write!(b, "<  {}  >", tr(lang, value));
                    txt(b.as_str())
                }
                _ => None,
            }
        },
        |g, ui, r| {
            super::scr::chrome(ui, screen, r);
            // Kick off: the reference's slanted lime pill.
            if let Some((mut a, size)) = art(r, ui, screen, "go", NO_ITEM) {
                let lit = g.cur.quick == QUICK_GO;
                a.slant(
                    Rect::new(0.0, 0.0, size.x, size.y),
                    if lit {
                        pal::LIME
                    } else {
                        Color::rgb(120, 160, 40)
                    },
                    5.0,
                );
            }
        },
    );
    // Each card: its club's rim, crest and kit.
    let ui = core::mem::take(&mut g.menu_ui);
    let cards = if big {
        [
            (
                "card_home_big",
                "crest_home_big",
                "kit_home_big",
                g.quick.home,
            ),
            (
                "card_away_big",
                "crest_away_big",
                "kit_away_big",
                g.quick.away,
            ),
        ]
    } else {
        [
            ("card_home", "crest_home", "kit_home", g.quick.home),
            ("card_away", "crest_away", "kit_away", g.quick.away),
        ]
    };
    for (card, crest, kit, c) in cards {
        let club = &CLUBS[c as usize];
        // The club's rim, inside the focus ring.
        if let Some((mut a, size)) = art(r, &ui, screen, card, NO_ITEM) {
            a.border(
                Rect::new(2.0, 2.0, size.x - 4.0, size.y - 4.0),
                rgb(club.home.shirt),
                1.0,
            );
        }
        if let Some((mut a, size)) = art(r, &ui, screen, crest, NO_ITEM) {
            crest_of(&mut a, 0.0, 0.0, size.x, c, true);
        }
        if let Some((mut a, _)) = art(r, &ui, screen, kit, NO_ITEM) {
            for (k, col) in [club.home.shirt, club.home.shorts, club.home.socks]
                .iter()
                .enumerate()
            {
                a.fill(Rect::new(k as f32 * 12.0, 0.0, 10.0, 8.0), rgb(*col));
            }
        }
    }
    g.menu_ui = ui;
    super::scr::hit(g, screen, cards[0].0, NO_ITEM, 0);
    super::scr::hit(g, screen, cards[1].0, NO_ITEM, 1);
    for k in 0..5u16 {
        super::scr::hit(g, screen, "row", k, k as u8 + 2);
    }
    super::scr::hit(g, screen, "go", NO_ITEM, QUICK_GO as u8);
    super::scr::back_hits(g, screen);
}

// --------------------------------------------------------------- pick club

fn pick_club(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, txt};
    const CARDS: [&str; TEAMS] = [
        "club0", "club1", "club2", "club3", "club4", "club5", "club6", "club7", "club8", "club9",
    ];
    const CRESTS: [&str; TEAMS] = [
        "crest0", "crest1", "crest2", "crest3", "crest4", "crest5", "crest6", "crest7", "crest8",
        "crest9",
    ];
    let screen = &super::pick_gen::SCREEN;
    let c = g.cur.pick.min(TEAMS - 1);
    super::scr::show(
        g,
        r,
        screen,
        Some((CARDS[c], NO_ITEM)),
        // The identity shows the pick.
        super::scr::rev(&(c, g.world.year)),
        |g, key, _| {
            let lang = g.settings.lang;
            if let Some(k) = key.strip_prefix("short") {
                return txt(CLUBS[k.parse::<usize>().ok()? % TEAMS].short);
            }
            let club = &CLUBS[c];
            match key {
                "name" => txt(upper::<20>(club.name).as_str()),
                "facts" => {
                    let mut b = buf();
                    let _ = write!(
                        b,
                        "{}  {} {}  {} {}",
                        FORMATIONS[club.tactics.formation as usize].name,
                        tr(lang, S::Ovr),
                        g.world.strength(c as u8),
                        tr(lang, S::Balance),
                        coins(g.world.clubs[c].budget).as_str()
                    );
                    txt(b.as_str())
                }
                _ => None,
            }
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            for id in ["ident", "ident_big"] {
                if let Some((mut a, size)) = art(r, ui, screen, id, NO_ITEM) {
                    let rect = Rect::new(0.0, 0.0, size.x, size.y);
                    a.panel(rect, rgb(CLUBS[c].home.shirt));
                }
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    for (k, id) in CRESTS.iter().enumerate() {
        if let Some((mut a, size)) = art(r, &ui, screen, id, NO_ITEM) {
            crest_of(&mut a, (size.x - 28.0) * 0.5, 0.0, 28.0, k as u8, false);
        }
    }
    let t = &CLUBS[c].tactics;
    for (k, v) in [t.press, t.line, t.tempo, t.direct].iter().enumerate() {
        const STYLE: [&str; 4] = ["style0", "style1", "style2", "style3"];
        const STYLE_BIG: [&str; 4] = ["style0_big", "style1_big", "style2_big", "style3_big"];
        let id = if g.big_text() { STYLE_BIG[k] } else { STYLE[k] };
        if let Some((mut a, size)) = art(r, &ui, screen, id, NO_ITEM) {
            a.bar(
                Rect::new(0.0, 0.0, size.x, size.y),
                *v as f32 / 100.0,
                pal::CYAN,
            );
        }
    }
    g.menu_ui = ui;
    for (k, id) in CARDS.iter().enumerate() {
        super::scr::hit(g, screen, id, NO_ITEM, k as u8);
    }
    super::scr::back_hits(g, screen);
}

// ----------------------------------------------------------------- club hub

fn club(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, flag, int, txt};
    let screen = &super::club_gen::SCREEN;
    let user = g.world.user;
    let big = g.access.clamped().text_factor() >= 1.5;
    let sfx = |id: &'static str, big_id: &'static str| if big { big_id } else { id };
    // The three best of the squad.
    let mut order: [u16; team::MAX_SQUAD] = [0; team::MAX_SQUAD];
    let mut n = 0;
    for i in g.world.squad(user) {
        if n < order.len() {
            order[n] = i as u16;
            n += 1;
        }
    }
    order[..n].sort_unstable_by_key(|&i| core::cmp::Reverse(g.world.players[i as usize].ovr()));
    let best = n.min(3);
    let season_over = g.world.round as usize >= ROUNDS;
    let acts: [S; 5] = if season_over {
        [S::Continue, S::Continue, S::Squad, S::Market, S::Table]
    } else {
        [S::Play, S::Simulate, S::Squad, S::Market, S::Table]
    };
    // The results played so far, oldest first (unplayed slots are 0).
    let form = g.world.form;
    let form_n = form.iter().filter(|&&f| f != 0).count();
    super::scr::show(
        g,
        r,
        screen,
        Some(match g.cur.club {
            5 => ("fin_big", NO_ITEM),
            6 => ("pl_big", NO_ITEM),
            7 => ("matches", NO_ITEM),
            k => (sfx("act", "act_big"), k.min(4) as u16),
        }),
        super::scr::rev(&(
            g.world.round,
            g.world.year,
            g.world.clubs[user as usize].budget,
            g.world.clubs[user as usize].morale,
            g.world.squad_len(user),
            form,
        )),
        |g, key, item| {
            let lang = g.settings.lang;
            let w = &g.world;
            let mut b = buf();
            match key {
                "balance" => {
                    let _ = write!(
                        b,
                        "{}: {} {}",
                        tr(lang, S::Balance),
                        coins(w.clubs[user as usize].budget).as_str(),
                        tr(lang, S::Coins)
                    );
                }
                "fame" => {
                    let _ = write!(
                        b,
                        "{}: {} {}",
                        tr(lang, S::Fame),
                        tr(lang, S::Level),
                        CLUBS[user as usize].fame
                    );
                }
                "income" => {
                    let _ = write!(
                        b,
                        "{}: +{}",
                        tr(lang, S::WeeklyIncome),
                        coins(w.last_income).as_str()
                    );
                }
                "wages" => {
                    let _ = write!(
                        b,
                        "{}: -{}",
                        tr(lang, S::Wages),
                        coins(w.last_wages).as_str()
                    );
                }
                "net_up" => return flag(w.last_income >= w.last_wages),
                "net_down" => return flag(w.last_income < w.last_wages),
                "squad" => {
                    let _ = write!(
                        b,
                        "{}/{}  {} {}",
                        w.squad_len(user),
                        team::MAX_SQUAD,
                        tr(lang, S::Morale),
                        w.clubs[user as usize].morale
                    );
                }
                "best" => return int(best as i32),
                "ovr" => {
                    return int(w.players[*order[..best].get(item as usize)? as usize].ovr() as i32);
                }
                "fixture" => return flag(w.user_fixture().is_some()),
                "over" => return flag(w.user_fixture().is_none() && season_over),
                "next" | "round" => {
                    let (h, a) = w.user_fixture()?;
                    let opp = if h == user { a } else { h };
                    if key == "next" {
                        let _ = write!(
                            b,
                            "{}: VS {}",
                            tr(lang, S::NextMatch),
                            CLUBS[opp as usize].short
                        );
                    } else {
                        let _ = write!(
                            b,
                            "{} {}/{}  {}",
                            tr(lang, S::Round),
                            w.round + 1,
                            ROUNDS,
                            tr(lang, if h == user { S::Home } else { S::Away })
                        );
                    }
                }
                "champion" => {
                    let leader = w.standings()[0];
                    let _ = write!(
                        b,
                        "{} {}",
                        CLUBS[leader as usize].short,
                        tr(lang, S::Champion)
                    );
                }
                "fitness" => {
                    let xi = w.lineup(user);
                    let fit: u32 = xi
                        .iter()
                        .map(|&i| w.players[i as usize].energy as u32)
                        .sum::<u32>()
                        / 11;
                    let _ = write!(b, "{}: {}%", tr(lang, S::Fitness), fit);
                }
                "form" => return int(form_n as i32),
                "win" | "draw" | "loss" => {
                    let f = form
                        .iter()
                        .copied()
                        .filter(|&f| f != 0)
                        .nth(item as usize)?;
                    return flag(match key {
                        "win" => f == 1,
                        "draw" => f == 2,
                        _ => f >= 3,
                    });
                }
                "acts" => return int(acts.len() as i32),
                "act" => return txt(tr(lang, *acts.get(item as usize)?)),
                _ => return None,
            }
            txt(b.as_str())
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            // The panels' fills under their colored rims.
            for id in ["fin", "pl", "fin_big", "pl_big", "matches"] {
                if let Some((mut a, size)) = art(r, ui, screen, id, NO_ITEM) {
                    a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
                }
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    let w = &g.world;
    if let Some((mut a, size)) = art(r, &ui, screen, "crest", NO_ITEM) {
        crest_of(&mut a, 0.0, 0.0, size.x, user, false);
    }
    // Every club's budget, the player's in lime.
    if let Some((mut a, size)) = art(r, &ui, screen, sfx("budgets", "budgets_big"), NO_ITEM) {
        let max = (0..TEAMS)
            .map(|c| w.clubs[c].budget.max(1))
            .max()
            .unwrap_or(1) as f32;
        let step = size.x / TEAMS as f32;
        for c in 0..TEAMS {
            let v = w.clubs[c].budget.max(0) as f32 / max;
            let h = (v * (size.y - 2.0)).max(1.0);
            let col = if c as u8 == user {
                pal::LIME
            } else {
                Color::rgb(60, 140, 80)
            };
            a.fill(
                Rect::new(c as f32 * step + 2.0, size.y - h, step - 4.0, h),
                col,
            );
        }
    }
    for k in 0..best {
        let i = order[k] as usize;
        if let Some((mut a, _)) = art(r, &ui, screen, sfx("portrait", "portrait_big"), k as u16) {
            portrait(&mut a, 0.0, 0.0, &look_of(g, i));
        }
        if let Some((mut a, size)) = art(r, &ui, screen, sfx("energy", "energy_big"), k as u16) {
            a.bar(
                Rect::new(0.0, 0.0, size.x, size.y),
                w.players[i].energy as f32 / 100.0,
                pal::GREEN,
            );
        }
    }
    if let Some((h, a2)) = w.user_fixture()
        && let Some((mut a, size)) = art(r, &ui, screen, "opp", NO_ITEM)
    {
        let opp = if h == user { a2 } else { h };
        crest_of(&mut a, 0.0, 0.0, size.x, opp, false);
    }
    if let Some((mut a, size)) = art(r, &ui, screen, "fit", NO_ITEM) {
        let xi = w.lineup(user);
        let fit: u32 = xi
            .iter()
            .map(|&i| w.players[i as usize].energy as u32)
            .sum::<u32>()
            / 11;
        a.bar(
            Rect::new(0.0, 0.0, size.x, size.y),
            fit as f32 / 100.0,
            pal::GREEN,
        );
    }
    g.menu_ui = ui;
    for k in 0..acts.len() {
        super::scr::hit(g, screen, sfx("act", "act_big"), k as u16, k as u8);
    }
    super::scr::back_hits(g, screen);
}

// ------------------------------------------------------------------ table

fn table(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, int, txt};
    let screen = &super::table_gen::SCREEN;
    let order = g.world.standings();
    let big = g.access.clamped().text_factor() >= 1.5;
    let (row, crest) = if big {
        ("row_big", "crest_big")
    } else {
        ("row", "crest")
    };
    super::scr::show(
        g,
        r,
        screen,
        Some((row, g.cur.table as u16)),
        super::scr::rev(&(g.world.round, g.world.year, g.world.user, g.career_on)),
        |g, key, item| {
            let lang = g.settings.lang;
            if key == "rows" {
                return int(TEAMS as i32);
            }
            let c = *order.get(item as usize)? as usize;
            let t = &g.world.table[c];
            let mut b = buf();
            match key {
                "pos" => int(item as i32 + 1),
                "name" => txt(upper::<20>(CLUBS[c].name).as_str()),
                "p" => int(t.played as i32),
                "w" => int(t.won as i32),
                "d" => int(t.drawn as i32),
                "l" => int(t.lost as i32),
                "gd" => int(t.gd() as i32),
                "pts" => int(t.points() as i32),
                "record" => {
                    let _ = write!(
                        b,
                        "P {}  W {}  D {}  L {}  GD {}",
                        t.played,
                        t.won,
                        t.drawn,
                        t.lost,
                        t.gd()
                    );
                    let _ = lang;
                    txt(b.as_str())
                }
                _ => None,
            }
        },
        |g, ui, r| {
            super::scr::chrome(ui, screen, r);
            if let Some((mut a, size)) = art(r, ui, screen, "panel", NO_ITEM) {
                a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
            }
            // The player's club, highlighted under its row.
            for (k, &c) in order.iter().enumerate() {
                if c == g.world.user
                    && g.career_on
                    && let Some((mut a, size)) = art(r, ui, screen, row, k as u16)
                {
                    a.fill(
                        Rect::new(-2.0, -1.0, size.x + 4.0, size.y + 1.0),
                        Color::rgba(30, 70, 140, 220),
                    );
                }
            }
        },
    );
    // The crests, over their boxes.
    let ui = core::mem::take(&mut g.menu_ui);
    for (k, &c) in order.iter().enumerate() {
        if let Some((mut a, size)) = art(r, &ui, screen, crest, k as u16) {
            crest_of(&mut a, 0.0, (size.y - 10.0) * 0.5, 10.0, c, false);
        }
    }
    g.menu_ui = ui;
    super::scr::back_hits(g, screen);
}

// ------------------------------------------------------------------ teams

fn stat_bars(ui: &mut Ui, x: f32, y: f32, p: &PlayerRec, lang: Lang, rows: &[(usize, S)]) {
    for (k, (a, s)) in rows.iter().enumerate() {
        let yy = y + k as f32 * 15.0;
        let mut b = buf();
        let _ = write!(b, "{}: {}", tr(lang, *s), p.attrs[*a]);
        ui.text(x, yy, b.as_str(), pal::INK, Size::Small, TextAlign::Left);
        let v = p.attrs[*a] as f32 / 100.0;
        let c = if p.attrs[*a] >= 80 {
            pal::GREEN
        } else if p.attrs[*a] >= 65 {
            pal::CYAN
        } else {
            pal::ORANGE
        };
        ui.bar(Rect::new(x, yy + 11.0, 66.0, 3.0), v, c);
    }
}

fn radar_of(ui: &mut Ui, cx: f32, cy: f32, rad: f32, p: &PlayerRec, fill: Color) {
    let v = |a: usize| p.attrs[a] as f32 / 100.0;
    ui.radar(
        cx,
        cy,
        rad,
        [
            v(team::SPEED),
            v(team::SHOT),
            v(team::DRIBBLE),
            v(team::DEFENSE),
            v(team::PASS),
        ],
        fill,
        pal::CYAN,
    );
}

fn teams(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, flag, int, txt};
    let screen = &super::teams_gen::SCREEN;
    let club = g.cur.team_club;
    let (list, n) = g.squad_list(club);
    let top = g.cur.squad_top;
    let shown = g.squad_rows().min(n.saturating_sub(top));
    let big = g.big_text();
    let row = if big { "row_big" } else { "row" };
    let sel = g.cur.squad.min(n.saturating_sub(1));
    let picked = (n > 0).then(|| g.world.players[list[sel] as usize]);
    const STATS: [(usize, S); 4] = [
        (team::SPEED, S::Speed),
        (team::SHOT, S::Shot),
        (team::PASS, S::Pass),
        (team::STAMINA, S::Stamina),
    ];
    let formation = g.world.clubs[club as usize].formation;
    super::scr::show(
        g,
        r,
        screen,
        Some((row, sel.saturating_sub(top) as u16)),
        super::scr::rev(&(club, top, sel, formation, picked.map(|p| p.attrs), n)),
        |g, key, item| {
            let lang = g.settings.lang;
            let f = FORMATIONS[formation as usize % FORMATIONS.len()];
            match key {
                "rows" => return int(shown as i32),
                "club" => {
                    if !g.big_text() {
                        return txt(CLUBS[club as usize].short);
                    }
                    let mut b = buf();
                    let _ = write!(b, "{} {}", CLUBS[club as usize].short, f.name);
                    return txt(b.as_str());
                }
                "stats_line" => {
                    let p = picked?;
                    let mut b = buf();
                    for (k, (a, s)) in STATS.iter().enumerate() {
                        let sep = if k > 0 { "  " } else { "" };
                        let _ = write!(b, "{sep}{} {}", tr(lang, *s), p.attrs[*a]);
                    }
                    return txt(b.as_str());
                }
                "formation" => return txt(f.name),
                "formation_line" => {
                    let mut b = buf();
                    let _ = write!(b, "{}: {}", tr(lang, S::Formation), f.name);
                    return txt(b.as_str());
                }
                _ => {}
            }
            if let Some(k) = key.strip_prefix("stat") {
                let (a, s) = STATS[k.parse::<usize>().ok()? % 4];
                let mut b = buf();
                let _ = write!(b, "{}: {}", tr(lang, s), picked?.attrs[a]);
                return txt(b.as_str());
            }
            let idx = top + item as usize;
            let p = g.world.players.get(*list[..n].get(idx)? as usize)?;
            match key {
                "name" => txt(name_line(p, lang).as_str()),
                "starter" => flag(idx < 11),
                "bench" => flag(idx >= 11),
                "ovr" => int(p.ovr() as i32),
                _ => None,
            }
        },
        |_, ui, r| super::scr::chrome(ui, screen, r),
    );
    let ui = core::mem::take(&mut g.menu_ui);
    if let Some((mut a, size)) = art(r, &ui, screen, "club_crest", NO_ITEM) {
        crest_of(&mut a, 0.0, 0.0, size.x, club, false);
    }
    // The rows' portraits and the starters' purple rims.
    let portrait_id = if big { "portrait_big" } else { "portrait" };
    for k in 0..shown {
        let i = list[top + k] as usize;
        if let Some((mut a, size)) = art(r, &ui, screen, row, k as u16)
            && top + k < 11
            && top + k != sel
        {
            a.border(Rect::new(0.0, 0.0, size.x, size.y), pal::PURPLE, 1.0);
        }
        if let Some((mut a, _)) = art(r, &ui, screen, portrait_id, k as u16) {
            portrait(&mut a, 0.0, 0.0, &look_of(g, i));
        }
    }
    if let Some(p) = picked {
        for (k, (attr, _)) in STATS.iter().enumerate() {
            const BARS: [&str; 4] = ["bar0", "bar1", "bar2", "bar3"];
            if let Some((mut a, size)) = art(r, &ui, screen, BARS[k], NO_ITEM) {
                let v = p.attrs[*attr];
                let c = if v >= 80 {
                    pal::GREEN
                } else if v >= 65 {
                    pal::CYAN
                } else {
                    pal::ORANGE
                };
                a.bar(Rect::new(0.0, 0.0, size.x, size.y), v as f32 / 100.0, c);
            }
        }
    }
    // The tactics board: the pitch and the eleven numbers.
    if let Some((mut a, size)) = art(r, &ui, screen, "board", NO_ITEM) {
        let (bw, bh) = (size.x, size.y);
        let pitch = Rect::new(0.0, 0.0, bw, bh);
        a.fill(pitch, Color::rgb(40, 130, 60));
        a.border(pitch, Color::rgba(230, 240, 230, 220), 1.0);
        a.fill(
            Rect::new(0.0, bh * 0.5, bw, 1.0),
            Color::rgba(230, 240, 230, 200),
        );
        a.border(
            Rect::new(bw * 0.25, bh - 22.0, bw * 0.5, 22.0),
            Color::rgba(230, 240, 230, 200),
            1.0,
        );
        let f = FORMATIONS[formation as usize % FORMATIONS.len()];
        let xi = g.world.lineup(club);
        for k in 0..11 {
            let b = f.base(k);
            // Normalized team space: x -1 (own goal, bottom) .. 0 (halfway).
            let px = (0.5 + b.y.to_f32() * 0.42) * bw;
            let py = bh * (0.5 - b.x.to_f32() * 0.46);
            a.fill(
                Rect::new(px - 6.0, py - 5.0, 12.0, 10.0),
                if k == 0 { pal::GREEN } else { pal::RED },
            );
            let mut nb = buf();
            let _ = write!(nb, "{}", g.world.players[xi[k] as usize].number);
            a.text(
                px,
                py - 5.0,
                nb.as_str(),
                pal::INK,
                Size::Small,
                TextAlign::Center,
            );
        }
    }
    g.menu_ui = ui;
    for k in 0..shown {
        super::scr::hit(g, screen, row, k as u16, (top + k) as u8);
    }
    super::scr::back_hits(g, screen);
}

// ------------------------------------------------------------------ editor

const EDIT_ROWS: [(usize, S); ATTRS] = [
    (team::SPEED, S::Speed),
    (team::DRIBBLE, S::Dribble),
    (team::SHOT, S::Shot),
    (team::PASS, S::Pass),
    (team::STAMINA, S::Stamina),
    (team::DEFENSE, S::Defense),
];

fn edit(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, flag, int, txt};
    let Some((i, _)) = g.edit else {
        return;
    };
    let screen = &super::edit_gen::SCREEN;
    let p = g.world.players[i];
    let big = g.big_text();
    let id = |small: &'static str, large: &'static str| if big { large } else { small };
    let row = g.cur.edit_attr;
    let focus = match row {
        k if k < ATTRS => (id("attr", "attr_big"), k as u16),
        k if k == ATTRS => (id("skin", "skin_big"), NO_ITEM),
        k if k == ATTRS + 1 => (id("style", "style_big"), NO_ITEM),
        _ => (id("hair", "hair_big"), NO_ITEM),
    };
    super::scr::show(
        g,
        r,
        screen,
        Some(focus),
        super::scr::rev(&(i, p.attrs, p.skin, p.style, p.hair, row)),
        |g, key, item| {
            let lang = g.settings.lang;
            let look = p.appearance();
            let mut b = buf();
            match key {
                "name" => return txt(upper::<16>(p.name()).as_str()),
                "attrs" => return int(ATTRS as i32),
                "label" => return txt(tr(lang, EDIT_ROWS.get(item as usize)?.1)),
                "value" => {
                    let (a, _) = EDIT_ROWS.get(item as usize)?;
                    if row == item as usize {
                        let _ = write!(b, "< {} >", p.attrs[*a]);
                    } else {
                        let _ = write!(b, "{}", p.attrs[*a]);
                    }
                }
                "ovr" => return int(p.ovr() as i32),
                "points" => return int(p.available() as i32),
                "points_left" => return flag(p.available() > 0),
                "no_points" => return flag(p.available() == 0),
                "style" => {
                    let style = tr(lang, S::hair_style(look.style as usize));
                    if row == ATTRS + 1 {
                        let _ = write!(b, "< {style} >");
                    } else {
                        let _ = write!(b, "{style}");
                    }
                }
                "summary" => {
                    let _ = write!(
                        b,
                        "{} {}  {} {}",
                        tr(lang, S::Ovr),
                        p.ovr(),
                        tr(lang, S::PointsLeft),
                        p.available()
                    );
                }
                _ => return None,
            }
            txt(b.as_str())
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            for panel in ["left", "right"] {
                if let Some((mut a, size)) = art(r, ui, screen, panel, NO_ITEM) {
                    a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
                }
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    for (k, (a, _)) in EDIT_ROWS.iter().enumerate() {
        if let Some((mut d, size)) = art(r, &ui, screen, id("abar", "abar_big"), k as u16) {
            d.bar(
                Rect::new(0.0, 0.0, size.x, size.y),
                p.attrs[*a] as f32 / 100.0,
                pal::CYAN,
            );
        }
    }
    if let Some((mut d, _)) = art(r, &ui, screen, id("skins", "skins_big"), NO_ITEM) {
        for k in 0..8 {
            let rr = Rect::new(k as f32 * 12.5, 0.0, 11.0, 16.0);
            d.fill(rr, SKIN_TONES[k]);
            if p.skin as usize == k {
                d.border(rr, pal::CYAN, 1.0);
            }
        }
    }
    if let Some((mut d, size)) = art(r, &ui, screen, "radar", NO_ITEM) {
        let rad = (size.y * 0.5 - 2.0).min(size.x * 0.5 - 2.0);
        radar_of(
            &mut d,
            size.x * 0.5,
            size.y * 0.5,
            rad,
            &p,
            Color::rgba(100, 220, 120, 210),
        );
    }
    // The hair colors; a tap on one picks it (before the row's own box:
    // the first box hit wins).
    let look = p.appearance();
    if let Some((mut d, _)) = art(r, &ui, screen, id("swatches", "swatches_big"), NO_ITEM) {
        for (k, &c) in render::HAIR_COLORS.iter().enumerate() {
            let rr = Rect::new(k as f32 * 17.5, 0.0, 15.0, 16.0);
            d.fill(rr, c);
            if look.hair as usize == k {
                d.border(rr, pal::CYAN, 1.0);
            }
            let hit = d.rect(rr);
            g.hits.add(hit, Screen::Top, (EDIT_ROW_COUNT + k) as u8);
        }
    }
    g.menu_ui = ui;
    for k in 0..ATTRS {
        super::scr::hit(g, screen, id("attr", "attr_big"), k as u16, k as u8);
    }
    super::scr::hit(g, screen, id("skin", "skin_big"), NO_ITEM, ATTRS as u8);
    super::scr::hit(
        g,
        screen,
        id("style", "style_big"),
        NO_ITEM,
        (ATTRS + 1) as u8,
    );
    super::scr::hit(
        g,
        screen,
        id("hair", "hair_big"),
        NO_ITEM,
        (ATTRS + 2) as u8,
    );
    super::scr::back_hits(g, screen);
}

// ------------------------------------------------------------------ market

fn market(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let (list, n) = g.market_list();
    let w = &g.world;
    let mut ui = Ui::new(r, screen);
    header(&mut ui, &mut g.hits, tr(lang, S::Transfers), true);
    let mut b = buf();
    let _ = write!(
        b,
        "{}: {} {}",
        tr(lang, S::Balance),
        coins(w.clubs[w.user as usize].budget).as_str(),
        tr(lang, S::Coins)
    );
    ui.slant(Rect::new(196.0, 6.0, 128.0, 14.0), pal::NAVY, 4.0);
    ui.text(
        312.0,
        7.0,
        b.as_str(),
        pal::YELLOW,
        Size::Small,
        TextAlign::Right,
    );
    ui.text(
        312.0,
        21.0,
        tr(lang, if g.cur.market_sell { S::Sell } else { S::Buy }),
        pal::LIME,
        Size::Small,
        TextAlign::Right,
    );
    // Sort keys.
    for (k, s) in [S::Price, S::Position, S::Ovr, S::Team].iter().enumerate() {
        let y = 36.0 + k as f32 * 20.0;
        let sel = g.cur.market_sort as usize == k;
        let rr = Rect::new(4.0, y, 44.0, 17.0);
        ui.fill(rr, pal::PANEL);
        ui.border(
            rr,
            if sel {
                pal::LIME
            } else {
                Color::rgb(60, 70, 110)
            },
            1.0,
        );
        ui.text(
            26.0,
            y + 3.0,
            tr(lang, *s),
            if sel { pal::LIME } else { pal::DIM },
            Size::Small,
            TextAlign::Center,
        );
    }
    // The list.
    for row in 0..5 {
        let k = g.cur.market_top + row;
        if k >= n {
            break;
        }
        let i = list[k] as usize;
        let p = &w.players[i];
        let y = 34.0 + row as f32 * 34.0;
        let sel = g.cur.market == k;
        let rr = Rect::new(52.0, y, 106.0, 32.0);
        ui.fill(
            rr,
            if sel {
                Color::rgba(40, 60, 20, 235)
            } else {
                pal::PANEL
            },
        );
        ui.border(
            rr,
            if sel {
                pal::LIME
            } else {
                Color::rgb(50, 60, 100)
            },
            1.0,
        );
        portrait(&mut ui, 55.0, y + 8.0, &look_of(g, i));
        let nl = name_line(p, lang);
        ui.text(
            74.0,
            y + 2.0,
            nl.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        let mut bb = buf();
        let _ = write!(bb, "{} {}", tr(lang, S::Ovr), p.ovr());
        ui.text(
            74.0,
            y + 12.0,
            bb.as_str(),
            pal::DIM,
            Size::Small,
            TextAlign::Left,
        );
        bb.clear();
        let price = if g.cur.market_sell {
            w.sell_price(i)
        } else {
            w.buy_price(i)
        };
        let _ = write!(bb, "{}", coins(price).as_str());
        ui.text(
            74.0,
            y + 21.0,
            bb.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Left,
        );
        ui.text(
            154.0,
            y + 21.0,
            CLUBS[w.club_of[i] as usize % TEAMS].short,
            pal::INK,
            Size::Small,
            TextAlign::Right,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, k as u8);
    }
    // The selected player's card.
    if n > 0 {
        let i = list[g.cur.market.min(n - 1)] as usize;
        let p = w.players[i];
        ui.panel(Rect::new(218.0, 34.0, 98.0, 160.0), Color::rgb(60, 70, 130));
        let name: TextBuf<16> = upper(p.name());
        ui.text(
            222.0,
            36.0,
            name.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        let mut bb = buf();
        let _ = write!(bb, "{}: {}", tr(lang, S::Position), pos_short(lang, p.pos));
        ui.text(
            222.0,
            48.0,
            bb.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Left,
        );
        bb.clear();
        let _ = write!(bb, "{}: {}", tr(lang, S::Age), p.age);
        ui.text(
            222.0,
            59.0,
            bb.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Left,
        );
        bb.clear();
        let _ = write!(
            bb,
            "{}: {}",
            tr(lang, S::Team),
            CLUBS[w.club_of[i] as usize % TEAMS].short
        );
        ui.text(
            222.0,
            70.0,
            bb.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Left,
        );
        stat_bars(
            &mut ui,
            222.0,
            84.0,
            &p,
            lang,
            &[
                (team::SPEED, S::Speed),
                (team::SHOT, S::Shot),
                (team::PASS, S::Pass),
                (team::DRIBBLE, S::Dribble),
            ],
        );
        ui.text(
            222.0,
            146.0,
            tr(lang, S::ForSale),
            pal::LIME,
            Size::Small,
            TextAlign::Left,
        );
        radar_of(
            &mut ui,
            268.0,
            174.0,
            15.0,
            &p,
            Color::rgba(220, 240, 90, 200),
        );
        // The big button.
        let price = if g.cur.market_sell {
            w.sell_price(i)
        } else {
            w.buy_price(i)
        };
        let mut bt = buf();
        let _ = write!(
            bt,
            "{} - {}",
            tr(lang, if g.cur.market_sell { S::Sell } else { S::Buy }),
            coins(price).as_str()
        );
        ui.slant(Rect::new(110.0, 204.0, 100.0, 16.0), pal::LIME, 4.0);
        ui.text(
            160.0,
            206.0,
            bt.as_str(),
            pal::NAVY,
            Size::Small,
            TextAlign::Center,
        );
    }
    footer(
        &mut ui,
        &mut g.hits,
        pad,
        lang,
        &[
            (Btn::A, if g.cur.market_sell { S::Sell } else { S::Buy }),
            (Btn::B, S::Back),
            (Btn::X, S::Compare),
            (Btn::Y, S::Swap),
        ],
    );
}

// ---------------------------------------------------------------- settings

fn settings(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    if g.scene == Scene::Confirm {
        let screen = &super::confirm_gen::SCREEN;
        let ids = ["cancel", "confirm"];
        super::scr::show(
            g,
            r,
            screen,
            Some((ids[g.cur.confirm.min(1)], NO_ITEM)),
            0,
            |_, _, _| None,
            |_, ui, r| super::scr::chrome(ui, screen, r),
        );
        for (k, id) in ids.iter().enumerate() {
            super::scr::hit(g, screen, id, NO_ITEM, k as u8);
        }
        super::scr::back_hits(g, screen);
        return;
    }
    let screen = &super::settings_gen::SCREEN;
    let rows = SolidElevenGame::settings_rows();
    super::scr::show(
        g,
        r,
        screen,
        Some(("row", g.cur.settings as u16)),
        super::scr::rev(&g.settings.bytes()),
        |g, key, item| settings_value(g, rows, key, item),
        |_, ui, r| super::scr::chrome(ui, screen, r),
    );
    for k in 0..rows.len() {
        super::scr::hit(g, screen, "row", k as u16, k as u8);
    }
    super::scr::back_hits(g, screen);
}

/// The settings screen's values: a row's label and its value between the
/// arrows that change it.
fn settings_value(
    g: &SolidElevenGame,
    rows: &[SettingRow],
    key: &str,
    item: u16,
) -> Option<trino_core::ui::UiValue> {
    use super::scr::{int, txt};
    let lang = g.settings.lang;
    let st = g.settings;
    if key == "rows" {
        return int(rows.len() as i32);
    }
    let row = rows.get(item as usize)?;
    let mut mins = TextBuf::<12>::new();
    tr_fmt(
        lang,
        S::Minutes,
        &[Arg::int("n", st.half_mins as i64)],
        &mut mins,
    );
    let (label, value): (S, &str) = match row {
        SettingRow::Language => (S::Language, st.lang.name()),
        SettingRow::Difficulty => (
            S::Difficulty,
            tr(
                lang,
                [S::Easy, S::Normal, S::Hard][st.difficulty as usize % 3],
            ),
        ),
        SettingRow::Length => (S::Length, mins.as_str()),
        SettingRow::Keeper => (
            S::Keeper,
            tr(lang, if st.keeper { S::KeeperYou } else { S::KeeperAi }),
        ),
        SettingRow::Graphics => (
            S::Graphics,
            tr(
                lang,
                if st.graphics == 0 {
                    S::GfxMax
                } else {
                    S::GfxRetro
                },
            ),
        ),
        SettingRow::Controls => (S::Controls, tr(lang, S::ControlsSub)),
    };
    match key {
        "label" => txt(tr(lang, label)),
        "value" => {
            let mut b = buf();
            if *row == SettingRow::Controls {
                let _ = write!(b, "{value}  >");
            } else {
                let _ = write!(b, "<  {value}  >");
            }
            txt(b.as_str())
        }
        _ => None,
    }
}

// ------------------------------------------------------------------ report

fn results(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, flag, int, txt};
    let Some(rep) = g.report else {
        return;
    };
    let screen = &super::results_gen::SCREEN;
    let st = rep.stats;
    let poss = (st[0].possession + st[1].possession).max(1);
    let rows: [(S, u32, u32); 6] = [
        (
            S::Possession,
            st[0].possession * 100 / poss,
            st[1].possession * 100 / poss,
        ),
        (S::Shots, st[0].shots as u32, st[1].shots as u32),
        (S::OnTarget, st[0].on_target as u32, st[1].on_target as u32),
        (S::Passes, st[0].passes as u32, st[1].passes as u32),
        (S::Fouls, st[0].fouls as u32, st[1].fouls as u32),
        (S::Corners, st[0].corners as u32, st[1].corners as u32),
    ];
    let goals = &rep.goals[..rep.n_goals.min(10) as usize];
    // The k-th goal of `side`, as "NAME 23'".
    let goal_of = |g: &SolidElevenGame, side: u8, k: usize| -> Option<Buf> {
        let &(db, _, minute) = goals.iter().filter(|x| x.1 == side).nth(k)?;
        let name: TextBuf<16> = upper(
            SURNAMES
                [g.world.players[db as usize % team::PLAYERS].surname as usize % SURNAMES.len()],
        );
        let mut b = buf();
        let _ = write!(b, "{} {}'", name.as_str(), minute);
        Some(b)
    };
    // What scrolls into view at large text: the score, the stat rows,
    // the scorers, the income.
    let focus = match g.cur.results {
        0 => ("score", NO_ITEM),
        k @ 1..=6 => ("stat", (k - 1) as u16),
        7 => ("goals", NO_ITEM),
        _ => (if rep.income >= 0 { "income" } else { "loss" }, NO_ITEM),
    };
    super::scr::show(
        g,
        r,
        screen,
        Some(focus),
        super::scr::rev(&(rep.score, rep.n_goals, rep.income)),
        |g, key, item| {
            let lang = g.settings.lang;
            let count = |side: u8| goals.iter().filter(|x| x.1 == side).count() as i32;
            match key {
                "home" => txt(CLUBS[rep.clubs[0] as usize].short),
                "away" => txt(CLUBS[rep.clubs[1] as usize].short),
                "score" => {
                    let mut b = buf();
                    let _ = write!(b, "{} - {}", rep.score[0], rep.score[1]);
                    txt(b.as_str())
                }
                "stats" => flag(st[0].passes + st[1].passes > 0),
                "rows" => int(rows.len() as i32),
                "label" => txt(tr(lang, rows.get(item as usize)?.0)),
                "a" => int(rows.get(item as usize)?.1 as i32),
                "b" => int(rows.get(item as usize)?.2 as i32),
                "home_goals" => int(count(0)),
                "away_goals" => int(count(1)),
                "home_goal" => txt(goal_of(g, 0, item as usize)?.as_str()),
                "away_goal" => txt(goal_of(g, 1, item as usize)?.as_str()),
                "gain" => flag(rep.career && rep.income >= 0),
                "loss" => flag(rep.career && rep.income < 0),
                "income" => {
                    let mut b = buf();
                    let _ = write!(
                        b,
                        "{}: {}",
                        tr(lang, S::Balance),
                        coins(rep.income).as_str()
                    );
                    txt(b.as_str())
                }
                _ => None,
            }
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            if let Some((mut a, size)) = art(r, ui, screen, "score", NO_ITEM) {
                a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    for (id, c) in [("crest_home", rep.clubs[0]), ("crest_away", rep.clubs[1])] {
        if let Some((mut a, size)) = art(r, &ui, screen, id, NO_ITEM) {
            crest_of(&mut a, 0.0, 0.0, size.x, c, false);
        }
    }
    // Two bars from the middle, each side's share.
    for (k, (_, a, b)) in rows.iter().enumerate() {
        if let Some((mut d, size)) = art(r, &ui, screen, "bars", k as u16) {
            let tot = (*a + *b).max(1) as f32;
            let half = (size.x - 8.0) * 0.5;
            d.bar(
                Rect::new(0.0, 0.0, half, size.y),
                *a as f32 / tot,
                pal::CYAN,
            );
            d.bar(
                Rect::new(half + 8.0, 0.0, half, size.y),
                *b as f32 / tot,
                pal::ORANGE,
            );
        }
    }
    g.menu_ui = ui;
}

/// The pause menu: a UI screen (`ui/pause.ui.ron`, issue #58) in the
/// game's language, laid out and scaled by the engine, at the player's
/// text scale and contrast.
fn pause(g: &mut SolidElevenGame, r: &mut dyn Renderer) {
    let strings = super::i18n::UiTr(g.settings.lang);
    let env = trino_core::UiEnv::of(r)
        .with_strings(&strings)
        .with_access(&g.access);
    g.pause_ui.draw(r, &super::pause_gen::SCREEN, &env);
}

fn half_time(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(0, 0, 0, 160),
    );
    ui.panel(Rect::new(45.0, 24.0, 230.0, 184.0), pal::CYAN);
    ui.text(
        160.0,
        30.0,
        tr(lang, S::HalfTime),
        pal::CYAN,
        Size::Big,
        TextAlign::Center,
    );
    if let Some(s) = &g.session {
        let [h, a] = s.m.score;
        let mut score_str = TextBuf::<32>::new();
        let _ = write!(score_str, "{}  -  {}", h, a);
        ui.text(
            160.0,
            52.0,
            score_str.as_str(),
            pal::INK,
            Size::Big,
            TextAlign::Center,
        );
        let h_shots = s.m.stats[0].shots as u32;
        let a_shots = s.m.stats[1].shots as u32;
        let h_fouls = s.m.stats[0].fouls as u32;
        let a_fouls = s.m.stats[1].fouls as u32;
        let h_corners = s.m.stats[0].corners as u32;
        let a_corners = s.m.stats[1].corners as u32;
        let tot_poss = (s.m.stats[0].possession + s.m.stats[1].possession).max(1);
        let h_poss = (s.m.stats[0].possession * 100) / tot_poss;
        let a_poss = 100 - h_poss;

        let stats: [(&str, u32, u32); 3] = [
            (tr(lang, S::Shots), h_shots, a_shots),
            (tr(lang, S::Fouls), h_fouls, a_fouls),
            (tr(lang, S::Corners), h_corners, a_corners),
        ];
        for (k, (lbl, v0, v1)) in stats.iter().enumerate() {
            let y = 78.0 + k as f32 * 17.0;
            let mut b0 = TextBuf::<12>::new();
            let _ = write!(b0, "{}", v0);
            let mut b1 = TextBuf::<12>::new();
            let _ = write!(b1, "{}", v1);
            ui.text(
                78.0,
                y,
                b0.as_str(),
                pal::CYAN,
                Size::Small,
                TextAlign::Right,
            );
            ui.text(160.0, y, lbl, pal::DIM, Size::Small, TextAlign::Center);
            ui.text(
                242.0,
                y,
                b1.as_str(),
                pal::CYAN,
                Size::Small,
                TextAlign::Left,
            );
        }
        let y = 78.0 + 3.0 * 17.0;
        let mut b0 = TextBuf::<12>::new();
        let _ = write!(b0, "{}%", h_poss);
        let mut b1 = TextBuf::<12>::new();
        let _ = write!(b1, "{}%", a_poss);
        ui.text(
            78.0,
            y,
            b0.as_str(),
            pal::CYAN,
            Size::Small,
            TextAlign::Right,
        );
        ui.text(
            160.0,
            y,
            tr(lang, S::Possession),
            pal::DIM,
            Size::Small,
            TextAlign::Center,
        );
        ui.text(
            242.0,
            y,
            b1.as_str(),
            pal::CYAN,
            Size::Small,
            TextAlign::Left,
        );
    }
    for (k, s) in [S::Continue, S::Substitute, S::Quit].iter().enumerate() {
        let rr = Rect::new(55.0 + k as f32 * 72.0, 168.0, 66.0, 24.0);
        let sel = g.cur.half_time == k;
        ui.fill(
            rr,
            if sel {
                Color::rgba(30, 60, 130, 240)
            } else {
                pal::PANEL
            },
        );
        ui.border(
            rr,
            if sel {
                pal::CYAN
            } else {
                Color::rgb(50, 60, 100)
            },
            1.0,
        );
        ui.text(
            rr.pos.x + rr.size.x * 0.5,
            rr.pos.y + 6.0,
            tr(lang, *s),
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, k as u8);
    }
    footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Select)]);
}

fn substitutions(
    g: &mut SolidElevenGame,
    r: &mut dyn Renderer,
    screen: Vec2,
    _from_half_time: bool,
) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(8, 10, 24, 235),
    );
    header(&mut ui, &mut g.hits, tr(lang, S::Substitute), true);

    let Some(s) = &g.session else {
        return;
    };
    let side = s.human.unwrap_or(0);
    let subs_used = s.m.subs[side];
    let max_subs = crate::sim::engine::MAX_SUBS;
    let subs_left = max_subs.saturating_sub(subs_used);

    // Subs left count at top right
    let mut sub_stat = TextBuf::<48>::new();
    let _ = write!(
        sub_stat,
        "{}: {}/{}",
        tr(lang, S::SubsRemaining),
        subs_left,
        max_subs
    );
    ui.text(
        312.0,
        12.0,
        sub_stat.as_str(),
        if subs_left > 0 { pal::CYAN } else { pal::RED },
        Size::Small,
        TextAlign::Right,
    );

    // On-pitch squad (left pane)
    let pitch_pane_sel = g.cur.sub_pane == 0;
    ui.text(
        12.0,
        32.0,
        tr(lang, S::OnPitch),
        pal::YELLOW,
        Size::Small,
        TextAlign::Left,
    );
    for slot in 0..SIDE {
        let i = side * SIDE + slot;
        let p = &s.m.players[i];
        let y = 44.0 + slot as f32 * 15.0;
        let rr = Rect::new(8.0, y, 148.0, 14.0);
        let is_cursor = pitch_pane_sel && g.cur.sub_pitch == slot;
        let is_locked = g.cur.sub_selected_pitch == Some(slot);
        ui.fill(
            rr,
            if is_locked {
                Color::rgba(160, 120, 20, 240)
            } else if is_cursor {
                Color::rgba(30, 60, 130, 240)
            } else {
                pal::PANEL
            },
        );
        ui.border(
            rr,
            if is_locked {
                pal::YELLOW
            } else if is_cursor {
                pal::CYAN
            } else {
                Color::rgb(40, 48, 80)
            },
            1.0,
        );

        // Player number
        let mut num_str = TextBuf::<8>::new();
        let _ = write!(num_str, "{}", p.number);
        ui.text(
            12.0,
            y + 2.0,
            num_str.as_str(),
            pal::DIM,
            Size::Small,
            TextAlign::Left,
        );

        // Role / Pos
        ui.text(
            28.0,
            y + 2.0,
            pos_short(lang, p.role.pos()),
            pal::CYAN,
            Size::Small,
            TextAlign::Left,
        );

        // Name
        let name_idx = s.names[i] as usize % SURNAMES.len();
        let name: TextBuf<16> = upper(SURNAMES[name_idx]);
        ui.text(
            50.0,
            y + 2.0,
            name.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );

        // Yellow card indicator
        if p.yellow > 0 {
            ui.fill(Rect::new(122.0, y + 3.0, 4.0, 7.0), pal::YELLOW);
        }

        // Stamina bar
        ui.bar(
            Rect::new(130.0, y + 4.0, 22.0, 5.0),
            p.stamina.to_f32(),
            pal::LIME,
        );

        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, slot as u8);
    }

    // Bench squad (right pane)
    let bench_n = s.m.bench_n[side] as usize;
    let bench_pane_sel = g.cur.sub_pane == 1;
    ui.text(
        168.0,
        32.0,
        tr(lang, S::Bench),
        pal::YELLOW,
        Size::Small,
        TextAlign::Left,
    );
    if bench_n == 0 {
        ui.text(
            238.0,
            80.0,
            tr(lang, S::NoSubsLeft),
            pal::DIM,
            Size::Small,
            TextAlign::Center,
        );
    } else {
        for k in 0..bench_n {
            let (db, number, attrs, is_gk) = s.m.bench[side][k];
            let y = 44.0 + k as f32 * 16.5;
            let rr = Rect::new(164.0, y, 148.0, 15.5);
            let is_cursor = bench_pane_sel && g.cur.sub_bench == k;
            ui.fill(
                rr,
                if is_cursor {
                    Color::rgba(30, 60, 130, 240)
                } else {
                    pal::PANEL
                },
            );
            ui.border(
                rr,
                if is_cursor {
                    pal::CYAN
                } else {
                    Color::rgb(40, 48, 80)
                },
                1.0,
            );

            // Number
            let mut num_str = TextBuf::<8>::new();
            let _ = write!(num_str, "{}", number);
            ui.text(
                168.0,
                y + 2.5,
                num_str.as_str(),
                pal::DIM,
                Size::Small,
                TextAlign::Left,
            );

            // Pos
            let pos_name = if is_gk {
                pos_short(lang, crate::sim::team::Pos::Gk)
            } else {
                pos_short(lang, crate::sim::team::Pos::Mf)
            };
            ui.text(
                184.0,
                y + 2.5,
                pos_name,
                pal::CYAN,
                Size::Small,
                TextAlign::Left,
            );

            // Name
            let name_idx =
                g.world.players[db as usize % team::PLAYERS].surname as usize % SURNAMES.len();
            let name: TextBuf<16> = upper(SURNAMES[name_idx]);
            ui.text(
                206.0,
                y + 2.5,
                name.as_str(),
                pal::INK,
                Size::Small,
                TextAlign::Left,
            );

            // Overall rating
            let sum: u32 = attrs.iter().map(|&a| a as u32).sum();
            let ovr = sum / ATTRS as u32;
            let mut ovr_str = TextBuf::<8>::new();
            let _ = write!(ovr_str, "{}", ovr);
            ui.text(
                304.0,
                y + 2.5,
                ovr_str.as_str(),
                pal::YELLOW,
                Size::Small,
                TextAlign::Right,
            );

            let hit = ui.rect(rr);
            g.hits.add(hit, Screen::Top, (20 + k) as u8);
        }
    }

    if g.cur.sub_selected_pitch.is_some() {
        footer(
            &mut ui,
            &mut g.hits,
            pad,
            lang,
            &[(Btn::A, S::Confirm), (Btn::B, S::Cancel)],
        );
    } else {
        footer(
            &mut ui,
            &mut g.hits,
            pad,
            lang,
            &[(Btn::A, S::Select), (Btn::B, S::Back)],
        );
    }
}

// ----------------------------------------------------------- touch screen

/// The 3DS's bottom screen: in a match, the radar, the formation and the
/// human side's stamina tiles (tap one to control him); elsewhere the
/// current screen's controls.
fn bottom(g: &mut SolidElevenGame, r: &mut dyn Renderer) {
    let lang = g.settings.lang;
    let size = r
        .screen_size(Screen::Bottom)
        .unwrap_or(Vec2::new(320.0, 240.0));
    let mut ui = Ui::new(r, size);
    ui.fill_screen(pal::NAVY);
    let Some(s) = (if matches!(
        g.scene,
        Scene::Match | Scene::Pause | Scene::HalfTime | Scene::Substitutions { .. }
    ) {
        g.session.as_ref()
    } else {
        None
    }) else {
        ui.text(
            160.0,
            100.0,
            "SOLID ELEVEN",
            pal::INK,
            Size::Big,
            TextAlign::Center,
        );
        ui.text(
            160.0,
            130.0,
            tr(lang, S::PressStart),
            pal::CYAN,
            Size::Small,
            TextAlign::Center,
        );
        return;
    };
    let m = &s.m;
    // Radar.
    let map = Rect::new(4.0, 4.0, 200.0, 130.0);
    ui.fill(map, Color::rgb(36, 110, 50));
    ui.border(map, Color::rgb(220, 235, 220), 1.0);
    ui.fill(Rect::new(104.0, 4.0, 1.0, 130.0), Color::rgb(220, 235, 220));
    for i in 0..N {
        let p = &m.players[i];
        if p.off {
            continue;
        }
        let x = 104.0 + p.pos.x.to_f32() / 52.5 * 100.0;
        let y = 69.0 + p.pos.y.to_f32() / 34.0 * 65.0;
        ui.fill(
            Rect::new(x - 2.0, y - 2.0, 4.0, 4.0),
            s.colors.shirt[side_of(i)],
        );
    }
    let bx = 104.0 + m.ball.pos.x.to_f32() / 52.5 * 100.0;
    let by = 69.0 + m.ball.pos.z.to_f32() / 34.0 * 65.0;
    ui.fill(Rect::new(bx - 1.5, by - 1.5, 3.0, 3.0), Color::WHITE);
    // Formation of the human side (tap to change it).
    let side = s.human.unwrap_or(0);
    let f = FORMATIONS[m.tactics[side].formation as usize % FORMATIONS.len()];
    let rr = Rect::new(210.0, 4.0, 106.0, 30.0);
    ui.panel(rr, pal::CYAN);
    ui.text(
        263.0,
        6.0,
        tr(lang, S::Formation),
        pal::DIM,
        Size::Small,
        TextAlign::Center,
    );
    ui.text(
        263.0,
        18.0,
        f.name,
        pal::YELLOW,
        Size::Small,
        TextAlign::Center,
    );
    g.hits.add(rr, Screen::Bottom, 100);
    let mut b = buf();
    let _ = write!(
        b,
        "{} {} - {} {}",
        CLUBS[m.clubs[0] as usize].short, m.score[0], m.score[1], CLUBS[m.clubs[1] as usize].short
    );
    ui.text(
        263.0,
        44.0,
        b.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    b.clear();
    let _ = write!(b, "{:02}:{:02}", m.minute(), m.second());
    ui.text(
        263.0,
        58.0,
        b.as_str(),
        pal::CYAN,
        Size::Small,
        TextAlign::Center,
    );
    // Stamina tiles.
    for k in 0..SIDE {
        let i = side * SIDE + k;
        let p = &m.players[i];
        let (col, row) = (k % 6, k / 6);
        let tile = Rect::new(
            4.0 + col as f32 * 52.0,
            140.0 + row as f32 * 48.0,
            49.0,
            44.0,
        );
        let active = s.view.guides.active == Some(i as u8);
        ui.fill(
            tile,
            if active {
                Color::rgb(30, 90, 150)
            } else {
                pal::NAVY_2
            },
        );
        ui.border(
            tile,
            if p.off {
                pal::RED
            } else if active {
                pal::CYAN
            } else {
                Color::rgb(60, 70, 120)
            },
            1.0,
        );
        let mut nb = buf();
        let _ = write!(nb, "{}", p.number);
        ui.text(
            tile.pos.x + 24.5,
            tile.pos.y + 4.0,
            nb.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
        let st = p.stamina.to_f32();
        let c = if st > 0.6 {
            pal::GREEN
        } else if st > 0.35 {
            pal::YELLOW
        } else {
            pal::RED
        };
        ui.bar(
            Rect::new(tile.pos.x + 4.0, tile.pos.y + 30.0, 41.0, 5.0),
            st,
            c,
        );
        g.hits.add(tile, Screen::Bottom, k as u8);
    }
    let _ = (Crest::Shield, PlayerRec::value as fn(&PlayerRec) -> u32);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::selftest::NullRenderer;

    #[test]
    fn footer_hints_never_overflow_screen() {
        let mut r = NullRenderer {
            caps: trino_core::Caps::PC,
        };
        let mut ui = Ui::new(&mut r, Vec2::new(320.0, 240.0));

        let hint_lists: &[&[(Btn, S)]] = &[
            &[(Btn::A, S::Change), (Btn::B, S::Back)],
            &[(Btn::A, S::Continue)],
            &[(Btn::A, S::Select)],
            &[(Btn::A, S::Confirm), (Btn::B, S::Back)],
            &[(Btn::A, S::Select), (Btn::B, S::Back), (Btn::Y, S::Table)],
            &[(Btn::B, S::Back)],
            &[
                (Btn::A, S::Details),
                (Btn::B, S::Back),
                (Btn::X, S::Formation),
            ],
            &[(Btn::A, S::Save_), (Btn::B, S::Cancel), (Btn::X, S::Random)],
            &[
                (Btn::A, S::Sell),
                (Btn::B, S::Back),
                (Btn::X, S::Compare),
                (Btn::Y, S::Swap),
            ],
            &[
                (Btn::A, S::Buy),
                (Btn::B, S::Back),
                (Btn::X, S::Compare),
                (Btn::Y, S::Swap),
            ],
            &[(Btn::A, S::Select), (Btn::B, S::Back)],
            &[(Btn::A, S::Confirm), (Btn::B, S::Cancel)],
        ];

        for lang in [Lang::En, Lang::Pt] {
            for pad in [false, true] {
                for list in hint_lists {
                    let mut x = 6.0;
                    for (btn, s) in *list {
                        let (letter, c) = btn_prompt(pad, *btn);
                        let w = ui.hint(x, 224.0, letter, c, tr(lang, *s));
                        assert!(w > 10.0, "hint width must be positive and bounded");
                        x += w + 4.0;
                    }
                    assert!(
                        x <= 316.0,
                        "Footer overflow in lang={:?}, pad={}: total width {} exceeds 316px",
                        lang,
                        pad,
                        x
                    );
                }
            }
        }
    }
}
