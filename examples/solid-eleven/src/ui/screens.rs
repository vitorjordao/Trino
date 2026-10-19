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
use crate::ui::i18n::{Lang, S, pos_short, tr};
use crate::ui::{Size, Ui, pal};
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
enum Btn {
    A,
    B,
    X,
    Y,
}

fn btn_prompt(pad: bool, b: Btn) -> (&'static str, Color) {
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
                Scene::Pause => pause(g, r, screen),
                Scene::HalfTime => half_time(g, r, screen),
                Scene::Substitutions { from_half_time } => {
                    substitutions(g, r, screen, from_half_time)
                }
                _ => {}
            }
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
            footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Change), (Btn::B, S::Back)]);
        }
        Scene::Results => {
            title_backdrop(g, r, aspect);
            results(g, r, screen);
        }
    }
    if let Some(t) = g.toast {
        let mut ui = Ui::new(r, screen);
        let s = tr(lang, t.text);
        let w = ui.measure(s, Size::Small) + 24.0;
        let c = if t.good { pal::LIME } else { pal::RED };
        ui.slant(
            Rect::new(160.0 - w * 0.5, 196.0, w, 16.0),
            Color::rgba(10, 12, 30, 235),
            4.0,
        );
        ui.slant_border(Rect::new(160.0 - w * 0.5, 196.0, w, 16.0), c, 4.0, 1.0);
        ui.text(160.0, 198.0, s, c, Size::Small, TextAlign::Center);
    }
    if r.on_screen(Screen::Bottom) {
        bottom(g, r);
    }
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
    let name: TextBuf<20> = upper(CLUBS[c as usize].name);
    ui.text(
        x + 33.0,
        y + 16.0,
        name.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let mut b = buf();
    let _ = write!(
        b,
        "{} {}",
        coins(g.world.clubs[c as usize].budget).as_str(),
        tr(lang, S::Coins)
    );
    ui.text(
        x + 33.0,
        y + 27.0,
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

fn club_card(ui: &mut Ui, x: f32, y: f32, c: u8, label: S, lang: Lang, g: &SolidElevenGame) {
    let club = &CLUBS[c as usize];
    ui.panel(Rect::new(x, y, 112.0, 74.0), rgb(club.home.shirt));
    ui.text(
        x + 56.0,
        y + 2.0,
        tr(lang, label),
        pal::DIM,
        Size::Small,
        TextAlign::Center,
    );
    crest_of(ui, x + 8.0, y + 16.0, 30.0, c, true);
    let name: TextBuf<20> = upper(club.name);
    ui.text(
        x + 44.0,
        y + 16.0,
        name.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    // Kit swatches.
    for (k, col) in [club.home.shirt, club.home.shorts, club.home.socks]
        .iter()
        .enumerate()
    {
        ui.fill(
            Rect::new(x + 44.0 + k as f32 * 12.0, y + 30.0, 10.0, 8.0),
            rgb(*col),
        );
    }
    let mut b = buf();
    let _ = write!(
        b,
        "{} {}   {}",
        tr(lang, S::Ovr),
        g.world.strength(c),
        FORMATIONS[g.world.clubs[c as usize].formation as usize % FORMATIONS.len()].name
    );
    ui.text(
        x + 44.0,
        y + 42.0,
        b.as_str(),
        pal::YELLOW,
        Size::Small,
        TextAlign::Left,
    );
    ui.text(
        x + 8.0,
        y + 58.0,
        "<",
        pal::CYAN,
        Size::Small,
        TextAlign::Left,
    );
    ui.text(
        x + 104.0,
        y + 58.0,
        ">",
        pal::CYAN,
        Size::Small,
        TextAlign::Right,
    );
}

fn quick(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(6, 8, 20, 130),
    );
    header(&mut ui, &mut g.hits, tr(lang, S::QuickMatch), true);
    club_card(&mut ui, 14.0, 34.0, g.quick.home, S::Home, lang, g);
    club_card(&mut ui, 194.0, 34.0, g.quick.away, S::Away, lang, g);
    ui.text_shadow(160.0, 58.0, "VS", pal::YELLOW, Size::Big, TextAlign::Center);
    let side = match (g.quick.side, lang) {
        (0, Lang::En) => "P1: HOME",
        (1, Lang::En) => "P1: AWAY",
        (_, Lang::En) => "CPU V CPU",
        (0, Lang::Pt) => "P1: CASA",
        (1, Lang::Pt) => "P1: FORA",
        (_, Lang::Pt) => "CPU X CPU",
    };
    let sky = [S::Day, S::Dusk, S::Night][g.quick.sky as usize % 3];
    let stadium =
        [S::StadiumArena, S::StadiumHarbour, S::StadiumPark][g.quick.stadium as usize % 3];
    let rows: [(S, &str); 5] = [
        (S::Team, side),
        (S::Stadium, tr(lang, stadium)),
        (S::KickoffTime, tr(lang, sky)),
        (
            S::Weather,
            tr(lang, if g.quick.rain { S::Rain } else { S::Clear }),
        ),
        (
            S::Difficulty,
            tr(
                lang,
                [S::Easy, S::Normal, S::Hard][g.settings.difficulty as usize % 3],
            ),
        ),
    ];
    for (k, (label, value)) in rows.iter().enumerate() {
        let id = k + 2;
        let y = 114.0 + k as f32 * 16.0;
        let sel = g.cur.quick == id;
        let rr = Rect::new(54.0, y, 212.0, 14.0);
        ui.fill(
            rr,
            if sel {
                Color::rgba(30, 60, 130, 235)
            } else {
                pal::PANEL
            },
        );
        if sel {
            ui.border(rr, pal::CYAN, 1.0);
        }
        ui.text(
            60.0,
            y + 1.0,
            tr(lang, *label),
            pal::DIM,
            Size::Small,
            TextAlign::Left,
        );
        let mut b = buf();
        let _ = write!(b, "<  {value}  >");
        ui.text(
            260.0,
            y + 1.0,
            b.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Right,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, id as u8);
    }
    // Highlight the club cards when selected.
    for (k, x) in [(0usize, 14.0f32), (1, 194.0)] {
        if g.cur.quick == k {
            ui.border(Rect::new(x - 1.0, 33.0, 114.0, 76.0), pal::CYAN, 1.0);
        }
        let hit = ui.rect(Rect::new(x, 34.0, 112.0, 74.0));
        g.hits.add(hit, Screen::Top, k as u8);
    }
    let sel = g.cur.quick == QUICK_GO;
    let go = Rect::new(96.0, 198.0, 128.0, 18.0);
    ui.slant(
        go,
        if sel {
            pal::LIME
        } else {
            Color::rgb(120, 160, 40)
        },
        5.0,
    );
    ui.text(
        160.0,
        201.0,
        tr(lang, S::KickOff),
        pal::NAVY,
        Size::Small,
        TextAlign::Center,
    );
    let hit = ui.rect(go);
    g.hits.add(hit, Screen::Top, QUICK_GO as u8);
    footer(
        &mut ui,
        &mut g.hits,
        pad,
        lang,
        &[(Btn::A, S::Confirm), (Btn::B, S::Back)],
    );
}

// --------------------------------------------------------------- pick club

fn pick_club(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(6, 8, 20, 150),
    );
    header(&mut ui, &mut g.hits, tr(lang, S::Career), true);
    for c in 0..TEAMS {
        let (col, row) = (c % 5, c / 5);
        let x = 12.0 + col as f32 * 60.0;
        let y = 36.0 + row as f32 * 66.0;
        let sel = g.cur.pick == c;
        let rr = Rect::new(x, y, 56.0, 62.0);
        ui.panel(
            rr,
            if sel {
                pal::CYAN
            } else {
                Color::rgba(60, 70, 120, 255)
            },
        );
        crest_of(&mut ui, x + 14.0, y + 6.0, 28.0, c as u8, false);
        ui.text(
            x + 28.0,
            y + 44.0,
            CLUBS[c].short,
            if sel { pal::YELLOW } else { pal::INK },
            Size::Small,
            TextAlign::Center,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, c as u8);
    }
    // The pick's identity.
    let c = g.cur.pick;
    let club = &CLUBS[c];
    ui.panel(Rect::new(12.0, 172.0, 296.0, 46.0), rgb(club.home.shirt));
    let name: TextBuf<20> = upper(club.name);
    ui.text(
        18.0,
        175.0,
        name.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let t = &club.tactics;
    let mut b = buf();
    let _ = write!(
        b,
        "{}  {} {}  {} {}",
        FORMATIONS[t.formation as usize].name,
        tr(lang, S::Ovr),
        g.world.strength(c as u8),
        tr(lang, S::Balance),
        coins(g.world.clubs[c].budget).as_str()
    );
    ui.text(
        18.0,
        187.0,
        b.as_str(),
        pal::YELLOW,
        Size::Small,
        TextAlign::Left,
    );
    // Style bars: press, line, width, tempo.
    for (k, (v, label)) in [
        (t.press, "PRESS"),
        (t.line, "LINE"),
        (t.tempo, "TEMPO"),
        (t.direct, "DIRECT"),
    ]
    .iter()
    .enumerate()
    {
        let x = 18.0 + k as f32 * 72.0;
        ui.text(x, 199.0, label, pal::DIM, Size::Small, TextAlign::Left);
        ui.bar(Rect::new(x, 211.0, 60.0, 3.0), *v as f32 / 100.0, pal::CYAN);
    }
    footer(
        &mut ui,
        &mut g.hits,
        pad,
        lang,
        &[(Btn::A, S::Confirm), (Btn::B, S::Back)],
    );
}

// ----------------------------------------------------------------- club hub

fn club(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let w = &g.world;
    let user = w.user;
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(6, 8, 20, 140),
    );
    // Header: crest, title, balance, fame.
    crest_of(&mut ui, 6.0, 4.0, 26.0, user, false);
    ui.slant(Rect::new(36.0, 4.0, 180.0, 14.0), pal::NAVY, 4.0);
    ui.text(
        42.0,
        5.0,
        tr(lang, S::ManageClub),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let mut b = buf();
    let _ = write!(
        b,
        "{}: {} {}",
        tr(lang, S::Balance),
        coins(w.clubs[user as usize].budget).as_str(),
        tr(lang, S::Coins)
    );
    ui.text(
        40.0,
        19.0,
        b.as_str(),
        pal::YELLOW,
        Size::Small,
        TextAlign::Left,
    );
    b.clear();
    let _ = write!(
        b,
        "{}: {} {}",
        tr(lang, S::Fame),
        tr(lang, S::Level),
        CLUBS[user as usize].fame
    );
    ui.text(
        312.0,
        19.0,
        b.as_str(),
        pal::CYAN,
        Size::Small,
        TextAlign::Right,
    );

    // FINANCES (blue): the last round's books and every club's budget.
    let (x, y) = (6.0, 34.0);
    ui.panel(Rect::new(x, y, 152.0, 92.0), pal::BLUE);
    ui.text(
        x + 4.0,
        y + 2.0,
        tr(lang, S::Finances),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let max = (0..TEAMS)
        .map(|c| w.clubs[c].budget.max(1))
        .max()
        .unwrap_or(1) as f32;
    for c in 0..TEAMS {
        let v = w.clubs[c].budget.max(0) as f32 / max;
        let h = (v * 34.0).max(1.0);
        let col = if c as u8 == user {
            pal::LIME
        } else {
            Color::rgb(60, 140, 80)
        };
        ui.fill(
            Rect::new(x + 6.0 + c as f32 * 14.0, y + 50.0 - h, 10.0, h),
            col,
        );
    }
    b.clear();
    let _ = write!(
        b,
        "{}: +{}",
        tr(lang, S::WeeklyIncome),
        coins(w.last_income).as_str()
    );
    ui.text(
        x + 4.0,
        y + 56.0,
        b.as_str(),
        pal::GREEN,
        Size::Small,
        TextAlign::Left,
    );
    b.clear();
    let _ = write!(
        b,
        "{}: -{}",
        tr(lang, S::Wages),
        coins(w.last_wages).as_str()
    );
    ui.text(
        x + 4.0,
        y + 67.0,
        b.as_str(),
        pal::RED,
        Size::Small,
        TextAlign::Left,
    );
    let net = w.last_income - w.last_wages;
    ui.text(
        x + 4.0,
        y + 78.0,
        tr(lang, if net >= 0 { S::Positive } else { S::Negative }),
        if net >= 0 { pal::GREEN } else { pal::RED },
        Size::Small,
        TextAlign::Left,
    );

    // PLAYERS (green): squad size, morale, the three best with energy.
    let (x, y) = (162.0, 34.0);
    ui.panel(Rect::new(x, y, 152.0, 92.0), pal::GREEN);
    ui.text(
        x + 4.0,
        y + 2.0,
        tr(lang, S::Players),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    b.clear();
    let _ = write!(
        b,
        "{}/{}  {} {}",
        w.squad_len(user),
        team::MAX_SQUAD,
        tr(lang, S::Morale),
        w.clubs[user as usize].morale
    );
    ui.text(
        x + 4.0,
        y + 14.0,
        b.as_str(),
        pal::DIM,
        Size::Small,
        TextAlign::Left,
    );
    let mut best = [0usize; 3];
    let mut n = 0;
    let mut order: [u16; team::MAX_SQUAD] = [0; team::MAX_SQUAD];
    for i in w.squad(user) {
        if n < order.len() {
            order[n] = i as u16;
            n += 1;
        }
    }
    order[..n].sort_unstable_by_key(|&i| core::cmp::Reverse(w.players[i as usize].ovr()));
    for k in 0..3.min(n) {
        best[k] = order[k] as usize;
    }
    for k in 0..3.min(n) {
        let p = &w.players[best[k]];
        let yy = y + 28.0 + k as f32 * 20.0;
        portrait(&mut ui, x + 4.0, yy, &look_of(g, best[k]));
        let mut bb = buf();
        let _ = write!(bb, "{}", p.ovr());
        ui.text(
            x + 28.0,
            yy + 2.0,
            bb.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        ui.bar(
            Rect::new(x + 46.0, yy + 6.0, 96.0, 4.0),
            p.energy as f32 / 100.0,
            pal::GREEN,
        );
    }

    // MATCHES (orange): next fixture, fitness, form; the actions.
    let (x, y) = (6.0, 130.0);
    ui.panel(Rect::new(x, y, 308.0, 88.0), pal::ORANGE);
    ui.text(
        x + 4.0,
        y + 2.0,
        tr(lang, S::Matches),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let season_over = w.round as usize >= ROUNDS;
    if let Some((h, a)) = w.user_fixture() {
        let opp = if h == user { a } else { h };
        crest_of(&mut ui, x + 8.0, y + 16.0, 26.0, opp, false);
        b.clear();
        let _ = write!(
            b,
            "{}: VS {}",
            tr(lang, S::NextMatch),
            CLUBS[opp as usize].short
        );
        ui.text(
            x + 40.0,
            y + 16.0,
            b.as_str(),
            pal::GREEN,
            Size::Small,
            TextAlign::Left,
        );
        b.clear();
        let _ = write!(
            b,
            "{} {}/{}  {}",
            tr(lang, S::Round),
            w.round + 1,
            ROUNDS,
            tr(lang, if h == user { S::Home } else { S::Away })
        );
        ui.text(
            x + 40.0,
            y + 27.0,
            b.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Left,
        );
    } else if season_over {
        let leader = w.standings()[0];
        let mut bb = buf();
        let _ = write!(
            bb,
            "{} {}",
            CLUBS[leader as usize].short,
            tr(lang, S::Champion)
        );
        ui.text(
            x + 10.0,
            y + 18.0,
            bb.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Left,
        );
    }
    let xi = w.lineup(user);
    let fit: u32 = xi
        .iter()
        .map(|&i| w.players[i as usize].energy as u32)
        .sum::<u32>()
        / 11;
    b.clear();
    let _ = write!(b, "{}: {}%", tr(lang, S::Fitness), fit);
    ui.text(
        x + 40.0,
        y + 38.0,
        b.as_str(),
        pal::DIM,
        Size::Small,
        TextAlign::Left,
    );
    ui.bar(
        Rect::new(x + 40.0, y + 50.0, 100.0, 4.0),
        fit as f32 / 100.0,
        pal::GREEN,
    );
    // Last results: chips.
    ui.text(
        x + 160.0,
        y + 16.0,
        tr(lang, S::LastResults),
        pal::DIM,
        Size::Small,
        TextAlign::Left,
    );
    for (k, &f) in w.form.iter().enumerate() {
        if f == 0 {
            continue;
        }
        let (s, c) = match f {
            1 => (S::Win, pal::GREEN),
            2 => (S::Draw, Color::rgb(140, 140, 150)),
            _ => (S::Loss, pal::RED),
        };
        let cx = x + 160.0 + k as f32 * 16.0;
        ui.fill(Rect::new(cx, y + 28.0, 13.0, 13.0), c);
        ui.text(
            cx + 6.5,
            y + 29.0,
            tr(lang, s),
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
    }
    // Actions.
    let acts: [S; 5] = if season_over {
        [S::Continue, S::Continue, S::Squad, S::Market, S::Table]
    } else {
        [S::Play, S::Simulate, S::Squad, S::Market, S::Table]
    };
    for (k, s) in acts.iter().enumerate() {
        let bx = x + 4.0 + k as f32 * 61.0;
        let rr = Rect::new(bx, y + 62.0, 58.0, 20.0);
        let sel = g.cur.club == k;
        ui.fill(
            rr,
            if sel {
                Color::rgba(90, 50, 10, 240)
            } else {
                pal::PANEL
            },
        );
        ui.border(
            rr,
            if sel {
                pal::ORANGE
            } else {
                Color::rgb(90, 70, 50)
            },
            1.0,
        );
        let label = tr(lang, *s);
        ui.text(
            bx + 29.0,
            y + 66.0,
            label,
            if sel { pal::YELLOW } else { pal::INK },
            Size::Small,
            TextAlign::Center,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, k as u8);
    }
    footer(
        &mut ui,
        &mut g.hits,
        pad,
        lang,
        &[
            (Btn::A, S::Select),
            (Btn::B, S::Back),
            (Btn::Y, S::Table),
        ],
    );
}

// ------------------------------------------------------------------ table

fn table(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let w = &g.world;
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(6, 8, 20, 160),
    );
    header(&mut ui, &mut g.hits, tr(lang, S::Table), true);
    let (x, y) = (24.0, 34.0);
    ui.panel(Rect::new(x, y, 272.0, 182.0), pal::CYAN);
    let cols = ["P", "W", "D", "L", "GD", ""];
    for (k, c) in cols.iter().enumerate() {
        ui.text(
            x + 148.0 + k as f32 * 20.0,
            y + 3.0,
            c,
            pal::DIM,
            Size::Small,
            TextAlign::Center,
        );
    }
    ui.text(
        x + 262.0,
        y + 3.0,
        tr(lang, S::Pts),
        pal::DIM,
        Size::Small,
        TextAlign::Right,
    );
    for (k, &c) in w.standings().iter().enumerate() {
        let row = &w.table[c as usize];
        let yy = y + 16.0 + k as f32 * 16.0;
        if c == w.user && g.career_on {
            ui.fill(
                Rect::new(x + 2.0, yy - 1.0, 268.0, 15.0),
                Color::rgba(30, 70, 140, 220),
            );
        }
        let mut b = buf();
        let _ = write!(b, "{}", k + 1);
        ui.text(
            x + 12.0,
            yy,
            b.as_str(),
            pal::DIM,
            Size::Small,
            TextAlign::Center,
        );
        crest_of(&mut ui, x + 22.0, yy, 10.0, c, false);
        let name: TextBuf<20> = upper(CLUBS[c as usize].name);
        ui.text(
            x + 36.0,
            yy,
            name.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        let vals = [
            row.played as i16,
            row.won as i16,
            row.drawn as i16,
            row.lost as i16,
            row.gd(),
        ];
        for (j, v) in vals.iter().enumerate() {
            b.clear();
            let _ = write!(b, "{v}");
            ui.text(
                x + 148.0 + j as f32 * 20.0,
                yy,
                b.as_str(),
                pal::INK,
                Size::Small,
                TextAlign::Center,
            );
        }
        b.clear();
        let _ = write!(b, "{}", row.points());
        ui.text(
            x + 262.0,
            yy,
            b.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Right,
        );
    }
    footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::B, S::Back)]);
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

fn teams(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let club = g.cur.team_club;
    let (list, n) = g.squad_list(club);
    let mut ui = Ui::new(r, screen);
    header(&mut ui, &mut g.hits, tr(lang, S::TeamSelect), true);
    // Club name and crest, top right.
    ui.slant(Rect::new(212.0, 6.0, 112.0, 22.0), pal::NAVY, 6.0);
    ui.text(
        276.0,
        10.0,
        CLUBS[club as usize].short,
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    crest_of(&mut ui, 296.0, 4.0, 20.0, club, false);
    // Squad list.
    ui.text_shadow(
        8.0,
        32.0,
        tr(lang, S::Squad),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    for row in 0..6 {
        let k = g.cur.squad_top + row;
        if k >= n {
            break;
        }
        let i = list[k] as usize;
        let p = &g.world.players[i];
        let y = 46.0 + row as f32 * 28.0;
        let sel = g.cur.squad == k;
        let rr = Rect::new(6.0, y, 112.0, 26.0);
        ui.fill(
            rr,
            if sel {
                Color::rgba(30, 150, 200, 230)
            } else {
                pal::PANEL
            },
        );
        ui.border(
            rr,
            if sel {
                pal::CYAN
            } else if k < 11 {
                pal::PURPLE
            } else {
                Color::rgb(50, 50, 80)
            },
            1.0,
        );
        portrait(&mut ui, 9.0, y + 5.0, &look_of(g, i));
        let nl = name_line(p, lang);
        ui.text(
            28.0,
            y + 2.0,
            nl.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        let mut b = buf();
        let _ = write!(b, "{}", p.ovr());
        ui.text(
            114.0,
            y + 13.0,
            b.as_str(),
            if k < 11 { pal::YELLOW } else { pal::DIM },
            Size::Small,
            TextAlign::Right,
        );
        if k < 11 {
            ui.text(
                28.0,
                y + 13.0,
                "*",
                pal::YELLOW,
                Size::Small,
                TextAlign::Left,
            );
        }
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, k as u8);
    }
    // Stats under the model.
    if n > 0 {
        let p = g.world.players[list[g.cur.squad.min(n - 1)] as usize];
        ui.fill(
            Rect::new(124.0, 158.0, 72.0, 62.0),
            Color::rgba(10, 12, 30, 170),
        );
        stat_bars(
            &mut ui,
            128.0,
            160.0,
            &p,
            lang,
            &[
                (team::SPEED, S::Speed),
                (team::SHOT, S::Shot),
                (team::PASS, S::Pass),
                (team::STAMINA, S::Stamina),
            ],
        );
    }
    // Tactics board.
    let (bx, by, bw, bh) = (226.0, 34.0, 88.0, 150.0);
    ui.text_shadow(
        bx,
        by - 2.0,
        tr(lang, S::Tactics),
        pal::INK,
        Size::Small,
        TextAlign::Left,
    );
    let pitch = Rect::new(bx, by + 10.0, bw, bh - 10.0);
    ui.fill(pitch, Color::rgb(40, 130, 60));
    ui.border(pitch, Color::rgba(230, 240, 230, 220), 1.0);
    ui.fill(
        Rect::new(bx, by + 10.0 + (bh - 10.0) * 0.5, bw, 1.0),
        Color::rgba(230, 240, 230, 200),
    );
    ui.border(
        Rect::new(bx + bw * 0.25, by + bh - 22.0, bw * 0.5, 22.0),
        Color::rgba(230, 240, 230, 200),
        1.0,
    );
    let f = FORMATIONS[g.world.clubs[club as usize].formation as usize % FORMATIONS.len()];
    let xi = g.world.lineup(club);
    for k in 0..11 {
        let b = f.base(k);
        // Normalized team space: x -1 (own goal, bottom) .. 0 (halfway).
        let px = bx + (0.5 + b.y.to_f32() * 0.42) * bw;
        let py = by + 10.0 + (bh - 10.0) * (0.5 - b.x.to_f32() * 0.46);
        let keeper = k == 0;
        ui.fill(
            Rect::new(px - 6.0, py - 5.0, 12.0, 10.0),
            if keeper { pal::GREEN } else { pal::RED },
        );
        let mut nb = buf();
        let _ = write!(nb, "{}", g.world.players[xi[k] as usize].number);
        ui.text(
            px,
            py - 5.0,
            nb.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
    }
    ui.text(
        bx + bw * 0.5,
        by + bh + 2.0,
        f.name,
        pal::YELLOW,
        Size::Small,
        TextAlign::Center,
    );
    footer(
        &mut ui,
        &mut g.hits,
        pad,
        lang,
        &[
            (Btn::A, S::Details),
            (Btn::B, S::Back),
            (Btn::X, S::Formation),
        ],
    );
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

fn edit(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let Some((i, _)) = g.edit else {
        return;
    };
    let p = g.world.players[i];
    let mut ui = Ui::new(r, screen);
    header(&mut ui, &mut g.hits, tr(lang, S::PlayerEdit), true);
    let name: TextBuf<16> = upper(p.name());
    ui.slant(Rect::new(212.0, 6.0, 112.0, 22.0), pal::NAVY, 6.0);
    ui.text(
        266.0,
        10.0,
        name.as_str(),
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    // Left: attributes.
    ui.panel(Rect::new(6.0, 34.0, 110.0, 176.0), pal::CYAN);
    for (k, (a, s)) in EDIT_ROWS.iter().enumerate() {
        let y = 38.0 + k as f32 * 24.0;
        let sel = g.cur.edit_attr == k;
        if sel {
            ui.fill(
                Rect::new(8.0, y - 1.0, 106.0, 22.0),
                Color::rgba(30, 90, 150, 230),
            );
        }
        let mut b = buf();
        let _ = write!(b, "{}", tr(lang, *s));
        ui.text(12.0, y, b.as_str(), pal::INK, Size::Small, TextAlign::Left);
        b.clear();
        if sel {
            let _ = write!(b, "< {} >", p.attrs[*a]);
        } else {
            let _ = write!(b, "{}", p.attrs[*a]);
        }
        ui.text(
            110.0,
            y,
            b.as_str(),
            pal::YELLOW,
            Size::Small,
            TextAlign::Right,
        );
        ui.bar(
            Rect::new(12.0, y + 13.0, 98.0, 4.0),
            p.attrs[*a] as f32 / 100.0,
            pal::CYAN,
        );
        let hit = ui.rect(Rect::new(8.0, y - 1.0, 106.0, 22.0));
        g.hits.add(hit, Screen::Top, k as u8);
    }
    // Skin tone row.
    let y = 38.0 + ATTRS as f32 * 24.0;
    let sel = g.cur.edit_attr == ATTRS;
    if sel {
        ui.fill(
            Rect::new(8.0, y - 1.0, 106.0, 26.0),
            Color::rgba(30, 90, 150, 230),
        );
    }
    for k in 0..8 {
        let rr = Rect::new(11.0 + k as f32 * 12.5, y + 4.0, 11.0, 16.0);
        ui.fill(rr, SKIN_TONES[k]);
        if p.skin as usize == k {
            ui.border(rr, pal::CYAN, 1.0);
        }
    }
    let hit = ui.rect(Rect::new(8.0, y - 1.0, 106.0, 26.0));
    g.hits.add(hit, Screen::Top, ATTRS as u8);
    // Right: overall and points left, the radar, the hair.
    ui.panel(Rect::new(214.0, 34.0, 100.0, 176.0), pal::CYAN);
    ui.text(
        222.0,
        38.0,
        tr(lang, S::Ovr),
        pal::DIM,
        Size::Small,
        TextAlign::Left,
    );
    let mut b = buf();
    let _ = write!(b, "{}", p.ovr());
    ui.text(
        222.0,
        48.0,
        b.as_str(),
        pal::INK,
        Size::Big,
        TextAlign::Left,
    );
    radar_of(
        &mut ui,
        264.0,
        96.0,
        24.0,
        &p,
        Color::rgba(100, 220, 120, 210),
    );
    ui.text(
        264.0,
        124.0,
        tr(lang, S::PointsLeft),
        pal::DIM,
        Size::Small,
        TextAlign::Center,
    );
    b.clear();
    let _ = write!(b, "{}", p.available());
    ui.text(
        264.0,
        134.0,
        b.as_str(),
        if p.available() > 0 {
            pal::YELLOW
        } else {
            pal::DIM
        },
        Size::Big,
        TextAlign::Center,
    );
    // Hair: the style by name, then the colors.
    let look = p.appearance();
    for (row, y) in [(ATTRS + 1, 158.0f32), (ATTRS + 2, 182.0)] {
        if g.cur.edit_attr == row {
            ui.fill(
                Rect::new(216.0, y - 1.0, 96.0, 24.0),
                Color::rgba(30, 90, 150, 230),
            );
        }
        if row == ATTRS + 2 {
            // A tap on a swatch picks that color (before the row's box:
            // the first box hit wins).
            for k in 0..render::HAIR_COLORS.len() {
                let hit = ui.rect(swatch(k));
                g.hits.add(hit, Screen::Top, (EDIT_ROW_COUNT + k) as u8);
            }
        }
        let hit = ui.rect(Rect::new(216.0, y - 1.0, 96.0, 24.0));
        g.hits.add(hit, Screen::Top, row as u8);
    }
    ui.text(
        222.0,
        158.0,
        tr(lang, S::Hair),
        pal::DIM,
        Size::Small,
        TextAlign::Left,
    );
    b.clear();
    let style = tr(lang, S::hair_style(look.style as usize));
    if g.cur.edit_attr == ATTRS + 1 {
        let _ = write!(b, "< {style} >");
    } else {
        let _ = write!(b, "{style}");
    }
    ui.text(
        264.0,
        168.0,
        b.as_str(),
        pal::YELLOW,
        Size::Small,
        TextAlign::Center,
    );
    for (k, &c) in render::HAIR_COLORS.iter().enumerate() {
        let rr = swatch(k);
        ui.fill(rr, c);
        if look.hair as usize == k {
            ui.border(rr, pal::CYAN, 1.0);
        }
    }
    footer(
        &mut ui,
        &mut g.hits,
        pad,
        lang,
        &[
            (Btn::A, S::Save_),
            (Btn::B, S::Cancel),
            (Btn::X, S::Random),
        ],
    );
}

/// The player editor's hair color swatch `k`.
fn swatch(k: usize) -> Rect {
    Rect::new(221.0 + k as f32 * 17.5, 186.0, 15.0, 16.0)
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

fn settings(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(6, 8, 20, 170),
    );
    if g.scene == Scene::Confirm {
        header(&mut ui, &mut g.hits, tr(lang, S::Exit), true);
        ui.panel(Rect::new(70.0, 80.0, 180.0, 70.0), pal::PINK);
        ui.text(
            160.0,
            86.0,
            tr(lang, S::ExitSub),
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
        for (k, s) in [S::Cancel, S::Confirm].iter().enumerate() {
            let rr = Rect::new(86.0 + k as f32 * 76.0, 116.0, 70.0, 20.0);
            let sel = g.cur.confirm == k;
            ui.fill(
                rr,
                if sel {
                    Color::rgba(110, 20, 60, 240)
                } else {
                    pal::PANEL
                },
            );
            ui.border(
                rr,
                if sel {
                    pal::PINK
                } else {
                    Color::rgb(80, 60, 90)
                },
                1.0,
            );
            ui.text(
                rr.pos.x + 35.0,
                120.0,
                tr(lang, *s),
                pal::INK,
                Size::Small,
                TextAlign::Center,
            );
            let hit = ui.rect(rr);
            g.hits.add(hit, Screen::Top, k as u8);
        }
        footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Select), (Btn::B, S::Back)]);
        return;
    }
    header(&mut ui, &mut g.hits, tr(lang, S::Settings), true);
    let st = g.settings;
    let mut mins = TextBuf::<12>::new();
    let _ = write!(mins, "{} {}", st.half_mins, tr(lang, S::Minutes));
    let rows = SolidElevenGame::settings_rows();
    for (k, row) in rows.iter().enumerate() {
        let (label, value): (S, &str) = match row {
            SettingRow::Language => (
                S::Language,
                if st.lang == Lang::En {
                    "ENGLISH"
                } else {
                    "PORTUGUÊS"
                },
            ),
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
        let y = 40.0 + k as f32 * 26.0;
        let sel = g.cur.settings == k;
        let rr = Rect::new(40.0, y, 240.0, 22.0);
        ui.fill(
            rr,
            if sel {
                Color::rgba(30, 60, 130, 235)
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
            48.0,
            y + 5.0,
            tr(lang, label),
            pal::DIM,
            Size::Small,
            TextAlign::Left,
        );
        let mut b = buf();
        if *row == SettingRow::Controls {
            let _ = write!(b, "{value}  >");
        } else {
            let _ = write!(b, "<  {value}  >");
        }
        ui.text(
            272.0,
            y + 5.0,
            b.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Right,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, k as u8);
    }
    footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Select), (Btn::B, S::Back)]);
}

// ------------------------------------------------------------------ report

fn results(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let pad = g.pad_layout();
    let Some(rep) = g.report else {
        return;
    };
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(6, 8, 20, 170),
    );
    header(&mut ui, &mut g.hits, tr(lang, S::Results), false);
    // Score line.
    ui.panel(Rect::new(40.0, 32.0, 240.0, 40.0), pal::CYAN);
    crest_of(&mut ui, 48.0, 37.0, 26.0, rep.clubs[0], false);
    crest_of(&mut ui, 246.0, 37.0, 26.0, rep.clubs[1], false);
    ui.text(
        96.0,
        40.0,
        CLUBS[rep.clubs[0] as usize].short,
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    ui.text(
        224.0,
        40.0,
        CLUBS[rep.clubs[1] as usize].short,
        pal::INK,
        Size::Small,
        TextAlign::Center,
    );
    let mut b = buf();
    let _ = write!(b, "{} - {}", rep.score[0], rep.score[1]);
    ui.text(
        160.0,
        36.0,
        b.as_str(),
        pal::YELLOW,
        Size::Big,
        TextAlign::Center,
    );
    // Stats: two bars from the middle.
    let st = &rep.stats;
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
    if st[0].passes + st[1].passes > 0 {
        for (k, (s, a, bv)) in rows.iter().enumerate() {
            let y = 80.0 + k as f32 * 17.0;
            ui.text(
                160.0,
                y,
                tr(lang, *s),
                pal::DIM,
                Size::Small,
                TextAlign::Center,
            );
            let tot = (*a + *bv).max(1) as f32;
            ui.bar(
                Rect::new(40.0, y + 11.0, 116.0, 3.0),
                *a as f32 / tot,
                pal::CYAN,
            );
            ui.bar(
                Rect::new(164.0, y + 11.0, 116.0, 3.0),
                *bv as f32 / tot,
                pal::ORANGE,
            );
            let mut nb = buf();
            let _ = write!(nb, "{a}");
            ui.text(40.0, y, nb.as_str(), pal::INK, Size::Small, TextAlign::Left);
            nb.clear();
            let _ = write!(nb, "{bv}");
            ui.text(
                280.0,
                y,
                nb.as_str(),
                pal::INK,
                Size::Small,
                TextAlign::Right,
            );
        }
    }
    // Scorers.
    for k in 0..rep.n_goals.min(4) as usize {
        let (db, side, minute) = rep.goals[k];
        let name: TextBuf<16> = upper(
            SURNAMES
                [g.world.players[db as usize % team::PLAYERS].surname as usize % SURNAMES.len()],
        );
        let mut nb = buf();
        let _ = write!(nb, "{} {}'", name.as_str(), minute);
        let x = if side == 0 { 44.0 } else { 276.0 };
        let align = if side == 0 {
            TextAlign::Left
        } else {
            TextAlign::Right
        };
        ui.text(
            x,
            184.0 + (k / 2) as f32 * 11.0 + (k % 2) as f32 * 0.0,
            nb.as_str(),
            pal::INK,
            Size::Small,
            align,
        );
    }
    if rep.career {
        let mut nb = buf();
        let _ = write!(
            nb,
            "{}: {}",
            tr(lang, S::Balance),
            coins(rep.income).as_str()
        );
        ui.text(
            160.0,
            206.0,
            nb.as_str(),
            if rep.income >= 0 {
                pal::GREEN
            } else {
                pal::RED
            },
            Size::Small,
            TextAlign::Center,
        );
    }
    footer(&mut ui, &mut g.hits, pad, lang, &[(Btn::A, S::Continue)]);
}

fn pause(g: &mut SolidElevenGame, r: &mut dyn Renderer, screen: Vec2) {
    let lang = g.settings.lang;
    let mut ui = Ui::new(r, screen);
    ui.fill(
        Rect::new(-200.0, 0.0, 720.0, 240.0),
        Color::rgba(0, 0, 0, 120),
    );
    ui.panel(Rect::new(90.0, 60.0, 140.0, 110.0), pal::CYAN);
    ui.text(
        160.0,
        64.0,
        tr(lang, S::Paused),
        pal::CYAN,
        Size::Small,
        TextAlign::Center,
    );
    for (k, s) in [S::Resume, S::Substitute, S::Quit].iter().enumerate() {
        let rr = Rect::new(100.0, 78.0 + k as f32 * 26.0, 120.0, 20.0);
        let sel = g.cur.pause == k;
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
            160.0,
            rr.pos.y + 4.0,
            tr(lang, *s),
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
        let hit = ui.rect(rr);
        g.hits.add(hit, Screen::Top, k as u8);
    }
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
            ui.text(78.0, y, b0.as_str(), pal::CYAN, Size::Small, TextAlign::Right);
            ui.text(160.0, y, lbl, pal::DIM, Size::Small, TextAlign::Center);
            ui.text(242.0, y, b1.as_str(), pal::CYAN, Size::Small, TextAlign::Left);
        }
        let y = 78.0 + 3.0 * 17.0;
        let mut b0 = TextBuf::<12>::new();
        let _ = write!(b0, "{}%", h_poss);
        let mut b1 = TextBuf::<12>::new();
        let _ = write!(b1, "{}%", a_poss);
        ui.text(78.0, y, b0.as_str(), pal::CYAN, Size::Small, TextAlign::Right);
        ui.text(
            160.0,
            y,
            tr(lang, S::Possession),
            pal::DIM,
            Size::Small,
            TextAlign::Center,
        );
        ui.text(242.0, y, b1.as_str(), pal::CYAN, Size::Small, TextAlign::Left);
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
        ui.text(
            50.0,
            y + 2.0,
            SURNAMES[name_idx],
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
            ui.text(
                206.0,
                y + 2.5,
                SURNAMES[name_idx],
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
            &[(Btn::A, S::Details), (Btn::B, S::Back), (Btn::X, S::Formation)],
            &[(Btn::A, S::Save_), (Btn::B, S::Cancel), (Btn::X, S::Random)],
            &[(Btn::A, S::Sell), (Btn::B, S::Back), (Btn::X, S::Compare), (Btn::Y, S::Swap)],
            &[(Btn::A, S::Buy), (Btn::B, S::Back), (Btn::X, S::Compare), (Btn::Y, S::Swap)],
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

