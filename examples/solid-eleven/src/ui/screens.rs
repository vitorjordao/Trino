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
use crate::sim::team::{self, ATTRS, Appearance, CLUBS, FORMATIONS, PlayerRec, SURNAMES, TEAMS};
use crate::ui::i18n::{Lang, S, pos_short, tr, tr_fmt};
use crate::ui::{Size, Ui, pal};
use trino_core::i18n::Arg;
use trino_core::ui::NO_ITEM;
use trino_core::{Color, Rect, Renderer, Screen, TextAlign, TextBuf, Vec2};

type Buf = TextBuf<64>;

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

/// Draw the current screen (top) and the 3DS's touch screen.
pub fn draw(g: &mut SolidElevenGame, r: &mut dyn Renderer) {
    g.hits.clear();
    let screen = g.screen;
    let aspect = screen.x / screen.y.max(1.0);
    let lang = g.settings.lang;
    match g.scene {
        Scene::Match | Scene::Pause | Scene::HalfTime | Scene::Substitutions { .. } => {
            let text_scale = g.access.clamped().text_factor();
            if let Some(s) = &g.session {
                s.render_scaled(r, screen, lang, text_scale);
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
        Scene::Trophy => trophy(g, r, aspect),
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
            crate::ui::controls::draw(g, r);
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
                "good" => flag(self.0.good && !self.0.warn),
                "bad" => flag(!self.0.good && !self.0.warn),
                "warn" => flag(self.0.warn),
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
fn trophy(g: &mut SolidElevenGame, r: &mut dyn Renderer, aspect: f32) {
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
    let screen = &super::trophy_gen::SCREEN;
    let user = g.world.user;
    let place = order.iter().position(|&c| c == user).unwrap_or(0) + 1;
    super::scr::show(
        g,
        r,
        screen,
        None,
        super::scr::rev(&(champ, place, g.world.year)),
        |g, key, _| {
            use super::scr::{flag, txt};
            let lang = g.settings.lang;
            let mut b = buf();
            match key {
                "season" => {
                    let name: TextBuf<20> = upper(CLUBS[champ as usize].name);
                    let _ = write!(
                        b,
                        "{}  {} {}",
                        name.as_str(),
                        tr(lang, S::Season),
                        g.world.year
                    );
                }
                "won" => return flag(place == 1),
                "lost" => return flag(place != 1),
                "place" => {
                    if place == 1 {
                        let _ = write!(b, "{}", tr(lang, S::LeagueWon));
                    } else {
                        let _ = write!(b, "{} {}/{}", tr(lang, S::FinishedAt), place, order.len());
                    }
                }
                _ => return None,
            }
            txt(b.as_str())
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            if let Some((mut a, size)) = super::scr::art(r, ui, screen, "banner", NO_ITEM) {
                let rect = Rect::new(0.0, 0.0, size.x, size.y);
                a.slant(rect, pal::NAVY, 6.0);
                a.slant_border(rect, pal::YELLOW, 6.0, 1.0);
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    if let Some((mut a, size)) = super::scr::art(r, &ui, screen, "crest", NO_ITEM) {
        crest_of(&mut a, 0.0, 0.0, size.x, champ, false);
    }
    g.menu_ui = ui;
}

// ------------------------------------------------------------------ title

fn title(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, flag, int, txt};
    const ITEMS: [(S, S, Color); 6] = [
        (S::PlayNow, S::QuickMatch, pal::CYAN),
        (S::Career, S::ManageClub, pal::RED),
        (S::Customize, S::CustomizeSub, pal::YELLOW),
        (S::Transfers, S::TransfersSub, pal::GREEN),
        (S::Settings, S::SettingsSub, pal::BLUE),
        (S::Exit, S::ExitSub, pal::PINK),
    ];
    let screen = &super::title_gen::SCREEN;
    let club = if g.career_on {
        g.world.user
    } else {
        g.quick.home
    };
    super::scr::show(
        g,
        r,
        screen,
        Some(("item", g.cur.title.min(5) as u16)),
        super::scr::rev(&(
            club,
            g.career_on,
            g.world.round,
            g.world.year,
            g.world.clubs[club as usize].budget,
        )),
        |g, key, item| {
            let lang = g.settings.lang;
            let mut b = buf();
            match key {
                "items" => return int(ITEMS.len() as i32),
                "main" => return txt(tr(lang, ITEMS.get(item as usize)?.0)),
                "sub" => return txt(tr(lang, ITEMS.get(item as usize)?.1)),
                "career" => return flag(g.career_on),
                "card_title" => {
                    return txt(tr(lang, if g.career_on { S::Club } else { S::QuickMatch }));
                }
                "club" => return txt(upper::<20>(CLUBS[club as usize].name).as_str()),
                "coins" => {
                    let _ = write!(
                        b,
                        "{} {}",
                        coins(g.world.clubs[club as usize].budget).as_str(),
                        tr(lang, S::Coins)
                    );
                }
                "round" => {
                    let _ = write!(
                        b,
                        "{} {}/{}  {}",
                        tr(lang, S::Round),
                        (g.world.round as usize + 1).min(ROUNDS),
                        ROUNDS,
                        g.world.year
                    );
                }
                "summary" => {
                    let _ = write!(
                        b,
                        "{}  {} {}",
                        upper::<20>(CLUBS[club as usize].name).as_str(),
                        coins(g.world.clubs[club as usize].budget).as_str(),
                        tr(lang, S::Coins)
                    );
                }
                _ => {
                    // `c0`..`c5`: the item's color picks its subtitle's.
                    let k = key.strip_prefix('c')?.parse::<u16>().ok()?;
                    return flag(k == item);
                }
            }
            txt(b.as_str())
        },
        |g, ui, r| {
            super::scr::chrome(ui, screen, r);
            // The items' slanted bars: the selected one longer, rimmed in
            // its color.
            for (k, (_, _, c)) in ITEMS.iter().enumerate() {
                if let Some((mut a, size)) = art(r, ui, screen, "item", k as u16) {
                    let sel = g.cur.title == k;
                    let w = if sel { size.x } else { size.x - 10.0 };
                    let bar = Rect::new(0.0, 0.0, w, size.y);
                    a.slant(
                        bar,
                        if sel {
                            Color::rgba(20, 40, 90, 245)
                        } else {
                            pal::PANEL
                        },
                        5.0,
                    );
                    a.slant(Rect::new(0.0, 0.0, 3.0, size.y), *c, 5.0);
                    if sel {
                        a.slant_border(bar, *c, 5.0, 1.0);
                    }
                }
            }
            if let Some((mut a, size)) = art(r, ui, screen, "card", NO_ITEM) {
                a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
            }
            if let Some((mut a, size)) = art(r, ui, screen, "card_title", NO_ITEM) {
                a.fill(
                    Rect::new(0.0, 0.0, size.x, size.y),
                    Color::rgba(30, 60, 120, 240),
                );
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    if let Some((mut a, size)) = art(r, &ui, screen, "card_crest", NO_ITEM) {
        crest_of(&mut a, 0.0, 0.0, size.x, club, false);
    }
    // The logo: a shield, the ball over its corner, the name.
    if let Some((mut a, _)) = art(r, &ui, screen, "logo", NO_ITEM) {
        let p = |x: f32, y: f32| Vec2::new(x - 10.0, y - 6.0);
        a.polygon(
            &[
                p(10.0, 8.0),
                p(78.0, 8.0),
                p(78.0, 40.0),
                p(44.0, 62.0),
                p(10.0, 40.0),
            ],
            pal::RED,
        );
        a.polygon(
            &[
                p(13.0, 11.0),
                p(75.0, 11.0),
                p(75.0, 39.0),
                p(44.0, 58.0),
                p(13.0, 39.0),
            ],
            Color::rgb(30, 70, 190),
        );
        a.polygon(
            &[
                p(74.0, 6.0),
                p(82.0, 9.0),
                p(85.0, 17.0),
                p(82.0, 25.0),
                p(74.0, 28.0),
                p(66.0, 25.0),
                p(63.0, 17.0),
                p(66.0, 9.0),
            ],
            Color::rgb(245, 245, 250),
        );
        a.fill(Rect::new(61.0, 8.0, 6.0, 6.0), Color::rgb(30, 30, 40));
        a.text(
            9.5,
            8.5,
            "SOLID",
            Color::rgb(20, 30, 90),
            Size::Big,
            TextAlign::Left,
        );
        a.text(8.0, 7.0, "SOLID", pal::INK, Size::Big, TextAlign::Left);
        a.text(
            7.5,
            32.5,
            "ELEVEN",
            Color::rgb(90, 10, 30),
            Size::Big,
            TextAlign::Left,
        );
        a.text(6.0, 31.0, "ELEVEN", pal::INK, Size::Big, TextAlign::Left);
    }
    g.menu_ui = ui;
    for k in 0..ITEMS.len() {
        super::scr::hit(g, screen, "item", k as u16, k as u8);
    }
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
                3 => S::SideTwo,
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
                "points" => return int(p.available()),
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

fn market(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{art, flag, int, txt};
    let screen = &super::market_gen::SCREEN;
    let (list, n) = g.market_list();
    let top = g.cur.market_top;
    let shown = g.market_rows().min(n.saturating_sub(top));
    let big = g.big_text();
    let row = if big { "row_big" } else { "row" };
    let sel = g.cur.market.min(n.saturating_sub(1));
    let picked = (n > 0).then(|| list[sel] as usize);
    let sell = g.cur.market_sell;
    const STATS: [(usize, S); 4] = [
        (team::SPEED, S::Speed),
        (team::SHOT, S::Shot),
        (team::PASS, S::Pass),
        (team::DRIBBLE, S::Dribble),
    ];
    const SORTS: [S; 4] = [S::Price, S::Position, S::Ovr, S::Team];
    let user = g.world.user;
    super::scr::show(
        g,
        r,
        screen,
        (n > 0).then_some((row, sel.saturating_sub(top) as u16)),
        super::scr::rev(&(
            top,
            sel,
            sell,
            g.cur.market_sort,
            n,
            g.world.clubs[user as usize].budget,
            picked,
        )),
        |g, key, item| {
            let lang = g.settings.lang;
            let w = &g.world;
            let price = |i: usize| {
                if sell {
                    w.sell_price(i)
                } else {
                    w.buy_price(i)
                }
            };
            let mut b = buf();
            match key {
                "rows" => return int(shown as i32),
                "any" => return flag(n > 0),
                "sorts" => return int(4),
                "sort" => return txt(tr(lang, *SORTS.get(item as usize)?)),
                "on" => return flag(g.cur.market_sort as u16 == item),
                "off" => return flag(g.cur.market_sort as u16 != item),
                "balance" => {
                    let _ = write!(
                        b,
                        "{}: {} {}",
                        tr(lang, S::Balance),
                        coins(w.clubs[user as usize].budget).as_str(),
                        tr(lang, S::Coins)
                    );
                    return txt(b.as_str());
                }
                "act_label" => return txt(tr(lang, if sell { S::Sell } else { S::Buy })),
                "mode" => {
                    let mode = tr(lang, if sell { S::Sell } else { S::Buy });
                    if !g.big_text() {
                        return txt(mode);
                    }
                    let sort = tr(lang, SORTS[g.cur.market_sort as usize % 4]);
                    let _ = write!(b, "{mode} - {sort}");
                    return txt(b.as_str());
                }
                _ => {}
            }
            if let Some(k) = key.strip_prefix("stat")
                && let Ok(k) = k.parse::<usize>()
            {
                let (a, s) = STATS[k % 4];
                let _ = write!(b, "{}: {}", tr(lang, s), w.players[picked?].attrs[a]);
                return txt(b.as_str());
            }
            match key {
                "card_name" => return txt(upper::<16>(w.players[picked?].name()).as_str()),
                "pos" => {
                    let p = &w.players[picked?];
                    let _ = write!(b, "{}: {}", tr(lang, S::Position), pos_short(lang, p.pos));
                    return txt(b.as_str());
                }
                "age" => {
                    let _ = write!(b, "{}: {}", tr(lang, S::Age), w.players[picked?].age);
                    return txt(b.as_str());
                }
                "team" => {
                    let i = picked?;
                    let _ = write!(
                        b,
                        "{}: {}",
                        tr(lang, S::Team),
                        CLUBS[w.club_of[i] as usize % TEAMS].short
                    );
                    return txt(b.as_str());
                }
                "deal" => {
                    let _ = write!(
                        b,
                        "{} - {}",
                        tr(lang, if sell { S::Sell } else { S::Buy }),
                        coins(price(picked?)).as_str()
                    );
                    return txt(b.as_str());
                }
                _ => {}
            }
            let i = *list[..n].get(top + item as usize)? as usize;
            let p = &w.players[i];
            match key {
                "is_sel" => flag(top + item as usize == sel),
                "row_stats" => {
                    let _ = write!(b, "{} {}", tr(lang, S::Age), p.age);
                    for (a, s) in STATS {
                        let _ = write!(b, "  {} {}", tr(lang, s), p.attrs[a]);
                    }
                    txt(b.as_str())
                }
                "name" => txt(name_line(p, lang).as_str()),
                "ovr" => {
                    let _ = write!(b, "{} {}", tr(lang, S::Ovr), p.ovr());
                    txt(b.as_str())
                }
                "price" => txt(coins(price(i)).as_str()),
                "club" => txt(CLUBS[w.club_of[i] as usize % TEAMS].short),
                _ => None,
            }
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            if let Some((mut a, size)) = art(r, ui, screen, "card", NO_ITEM) {
                a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
            }
            if let Some((mut a, size)) = art(r, ui, screen, "deal", NO_ITEM) {
                a.slant(Rect::new(0.0, 0.0, size.x, size.y), pal::LIME, 4.0);
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    let portrait_id = if big { "portrait_big" } else { "portrait" };
    for k in 0..shown {
        let i = list[top + k] as usize;
        if let Some((mut a, _)) = art(r, &ui, screen, portrait_id, k as u16) {
            portrait(&mut a, 0.0, 0.0, &look_of(g, i));
        }
    }
    if let Some(i) = picked {
        let p = g.world.players[i];
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
        if let Some((mut a, size)) = art(r, &ui, screen, "radar", NO_ITEM) {
            let rad = (size.y * 0.5 - 1.0).min(size.x * 0.5 - 1.0);
            radar_of(
                &mut a,
                size.x * 0.5,
                size.y * 0.5,
                rad,
                &p,
                Color::rgba(220, 240, 90, 200),
            );
        }
    }
    g.menu_ui = ui;
    for k in 0..shown {
        super::scr::hit(g, screen, row, k as u16, (top + k) as u8);
    }
    super::scr::back_hits(g, screen);
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

fn half_time(g: &mut SolidElevenGame, r: &mut dyn Renderer, _screen: Vec2) {
    use super::scr::{int, txt};
    let Some(m) = g.session.as_ref().map(|s| (s.m.score, s.m.stats)) else {
        return;
    };
    let (score, st) = m;
    let screen = &super::half_time_gen::SCREEN;
    const IDS: [&str; 3] = ["continue", "substitute", "quit"];
    let tot = (st[0].possession + st[1].possession).max(1);
    let poss = (st[0].possession * 100) / tot;
    let rows: [(S, u32, u32, bool); 4] = [
        (S::Shots, st[0].shots as u32, st[1].shots as u32, false),
        (S::Fouls, st[0].fouls as u32, st[1].fouls as u32, false),
        (
            S::Corners,
            st[0].corners as u32,
            st[1].corners as u32,
            false,
        ),
        (S::Possession, poss, 100 - poss, true),
    ];
    super::scr::show(
        g,
        r,
        screen,
        Some((IDS[g.cur.half_time.min(2)], NO_ITEM)),
        super::scr::rev(&score),
        |g, key, item| {
            let lang = g.settings.lang;
            let mut b = buf();
            match key {
                "score" => {
                    let _ = write!(b, "{}  -  {}", score[0], score[1]);
                    txt(b.as_str())
                }
                "rows" => int(rows.len() as i32),
                "label" => txt(tr(lang, rows.get(item as usize)?.0)),
                "a" | "b" => {
                    let (_, a, bv, pct) = *rows.get(item as usize)?;
                    let v = if key == "a" { a } else { bv };
                    let _ = write!(b, "{v}{}", if pct { "%" } else { "" });
                    txt(b.as_str())
                }
                _ => None,
            }
        },
        |_, ui, r| {
            if let Some((mut a, size)) = super::scr::art(r, ui, screen, "panel", NO_ITEM) {
                a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
            }
            super::scr::chrome(ui, screen, r);
        },
    );
    for (k, id) in IDS.iter().enumerate() {
        super::scr::hit(g, screen, id, NO_ITEM, k as u8);
    }
}

fn substitutions(
    g: &mut SolidElevenGame,
    r: &mut dyn Renderer,
    _screen: Vec2,
    _from_half_time: bool,
) {
    use super::scr::{art, flag, int, txt};
    let screen = &super::substitutions_gen::SCREEN;
    let Some(s) = &g.session else {
        return;
    };
    let side = s.human.unwrap_or(0);
    let subs_left = crate::sim::engine::MAX_SUBS.saturating_sub(s.m.subs[side]);
    let bench_n = s.m.bench_n[side] as usize;
    let big = g.big_text();
    let (pitch_id, bench_id, meta_id) = if big {
        ("pitch_big", "bench_big", "meta_big")
    } else {
        ("pitch", "bench", "meta")
    };
    let focus = if g.cur.sub_pane == 0 {
        (pitch_id, g.cur.sub_pitch as u16)
    } else {
        (bench_id, g.cur.sub_bench as u16)
    };
    let locked = g.cur.sub_selected_pitch;
    super::scr::show(
        g,
        r,
        screen,
        Some(focus),
        super::scr::rev(&(
            g.cur.sub_pane,
            locked,
            s.m.subs[side],
            bench_n,
            s.m.bench[side].map(|b| b.0),
        )),
        |g, key, item| {
            let lang = g.settings.lang;
            let s = g.session.as_ref()?;
            let mut b = buf();
            match key {
                "left" => {
                    let _ = write!(
                        b,
                        "{}: {}/{}",
                        tr(lang, S::SubsRemaining),
                        subs_left,
                        crate::sim::engine::MAX_SUBS
                    );
                    return txt(b.as_str());
                }
                "subs_ok" => return flag(subs_left > 0),
                "subs_out" => return flag(subs_left == 0),
                "pane_pitch" => return flag(g.cur.sub_pane == 0),
                "pane_bench" => return flag(g.cur.sub_pane == 1),
                "pitch_n" => return int(SIDE as i32),
                "bench_n" => return int(bench_n as i32),
                "no_bench" => return flag(bench_n == 0),
                "a_label" => {
                    return txt(tr(
                        lang,
                        if locked.is_some() {
                            S::Confirm
                        } else {
                            S::Select
                        },
                    ));
                }
                "b_label" => {
                    return txt(tr(lang, if locked.is_some() { S::Cancel } else { S::Back }));
                }
                _ => {}
            }
            let k = item as usize;
            match key {
                "p_num" | "p_name" => {
                    let i = side * SIDE + k.min(SIDE - 1);
                    let p = &s.m.players[i];
                    if key == "p_num" {
                        let _ = write!(b, "{} {}", p.number, pos_short(lang, p.role.pos()));
                        txt(b.as_str())
                    } else {
                        txt(upper::<16>(SURNAMES[s.names[i] as usize % SURNAMES.len()]).as_str())
                    }
                }
                "b_num" | "b_name" | "b_ovr" => {
                    let (db, number, attrs, is_gk) = *s.m.bench[side][..bench_n].get(k)?;
                    match key {
                        "b_num" => {
                            let pos = if is_gk {
                                crate::sim::team::Pos::Gk
                            } else {
                                crate::sim::team::Pos::Mf
                            };
                            let _ = write!(b, "{} {}", number, pos_short(lang, pos));
                            txt(b.as_str())
                        }
                        "b_name" => {
                            let name = g.world.players[db as usize % team::PLAYERS].surname;
                            txt(upper::<16>(SURNAMES[name as usize % SURNAMES.len()]).as_str())
                        }
                        _ => {
                            let sum: u32 = attrs.iter().map(|&a| a as u32).sum();
                            int((sum / ATTRS as u32) as i32)
                        }
                    }
                }
                _ => None,
            }
        },
        |_, ui, r| {
            super::scr::chrome(ui, screen, r);
            // The rows' fills: the player picked to go off in gold.
            for k in 0..SIDE {
                if let Some((mut a, size)) = art(r, ui, screen, pitch_id, k as u16) {
                    let c = if locked == Some(k) {
                        Color::rgba(160, 120, 20, 240)
                    } else {
                        pal::PANEL
                    };
                    a.fill(Rect::new(0.0, 0.0, size.x, size.y), c);
                }
            }
            for k in 0..bench_n {
                if let Some((mut a, size)) = art(r, ui, screen, bench_id, k as u16) {
                    a.fill(Rect::new(0.0, 0.0, size.x, size.y), pal::PANEL);
                }
            }
        },
    );
    // A yellow card and the stamina left, per player on the pitch.
    let ui = core::mem::take(&mut g.menu_ui);
    if let Some(s) = &g.session {
        for k in 0..SIDE {
            let p = &s.m.players[side * SIDE + k];
            if let Some((mut a, size)) = art(r, &ui, screen, meta_id, k as u16) {
                if p.yellow > 0 {
                    a.fill(Rect::new(0.0, 0.0, 4.0, size.y), pal::YELLOW);
                }
                a.bar(
                    Rect::new(8.0, 1.0, size.x - 8.0, size.y - 2.0),
                    p.stamina.to_f32(),
                    pal::LIME,
                );
            }
        }
    }
    g.menu_ui = ui;
    for k in 0..SIDE {
        super::scr::hit(g, screen, pitch_id, k as u16, k as u8);
    }
    for k in 0..bench_n {
        super::scr::hit(g, screen, bench_id, k as u16, (20 + k) as u8);
    }
    super::scr::back_hits(g, screen);
}

// ----------------------------------------------------------- touch screen

/// The 3DS's bottom screen: in a match, the radar, the formation and the
/// human side's stamina tiles (tap one to control him); elsewhere the
/// current screen's controls.
fn bottom(g: &mut SolidElevenGame, r: &mut dyn Renderer) {
    use super::scr::{art, flag, txt};
    const TILES: [&str; SIDE] = [
        "tile0", "tile1", "tile2", "tile3", "tile4", "tile5", "tile6", "tile7", "tile8", "tile9",
        "tile10",
    ];
    const BARS: [&str; SIDE] = [
        "stamina0",
        "stamina1",
        "stamina2",
        "stamina3",
        "stamina4",
        "stamina5",
        "stamina6",
        "stamina7",
        "stamina8",
        "stamina9",
        "stamina10",
    ];
    let screen = &super::bottom_gen::SCREEN;
    let session = if matches!(
        g.scene,
        Scene::Match | Scene::Pause | Scene::HalfTime | Scene::Substitutions { .. }
    ) {
        g.session.as_ref()
    } else {
        None
    };
    struct Bottom<'a> {
        s: Option<&'a crate::game::session::Session>,
        lang: Lang,
    }
    impl trino_core::ui::UiData for Bottom<'_> {
        fn value(&self, key: &str, _: u16) -> Option<trino_core::ui::UiValue> {
            let mut b = buf();
            match key {
                "match" => return flag(self.s.is_some()),
                "menu" => return flag(self.s.is_none()),
                _ => {}
            }
            let s = self.s?;
            let m = &s.m;
            let side = s.human.unwrap_or(0);
            match key {
                "formation" => {
                    return txt(
                        FORMATIONS[m.tactics[side].formation as usize % FORMATIONS.len()].name,
                    );
                }
                "score" => {
                    let _ = write!(
                        b,
                        "{} {} - {} {}",
                        CLUBS[m.clubs[0] as usize].short,
                        m.score[0],
                        m.score[1],
                        CLUBS[m.clubs[1] as usize].short
                    );
                }
                "clock" => {
                    let _ = write!(b, "{:02}:{:02}", m.minute(), m.second());
                }
                _ => {
                    let k = key.strip_prefix("num")?.parse::<usize>().ok()?;
                    let _ = write!(b, "{}", m.players[side * SIDE + k.min(SIDE - 1)].number);
                }
            }
            let _ = self.lang;
            txt(b.as_str())
        }

        fn revision(&self) -> Option<u32> {
            let s = self.s?;
            let m = &s.m;
            Some(super::scr::rev(&(
                m.score,
                m.minute(),
                m.second(),
                m.tactics[s.human.unwrap_or(0)].formation,
                self.lang,
            )))
        }
    }
    let data = Bottom {
        s: session,
        lang: g.settings.lang,
    };
    let strings = super::i18n::UiTr(g.settings.lang);
    let env = trino_core::UiEnv::of(r)
        .with_strings(&strings)
        .with_access(&g.access)
        .with_data(&data);
    let mut ui = core::mem::take(&mut g.bottom_ui);
    let size = r
        .screen_size(Screen::Bottom)
        .unwrap_or(Vec2::new(320.0, 240.0));
    r.fill_rect(Rect::new(0.0, 0.0, size.x, size.y), pal::NAVY);
    ui.prepare(r, screen, &env);
    // The tiles' fills: the controlled player's in blue, sent-off in red.
    if let Some(s) = session {
        let side = s.human.unwrap_or(0);
        for k in 0..SIDE {
            if let Some((mut a, size)) = art(r, &ui, screen, TILES[k], NO_ITEM) {
                let i = side * SIDE + k;
                let active = s.view.guides.active == Some(i as u8);
                let rect = Rect::new(0.0, 0.0, size.x, size.y);
                a.fill(
                    rect,
                    if active {
                        Color::rgb(30, 90, 150)
                    } else {
                        pal::NAVY_2
                    },
                );
                let rim = if s.m.players[i].off {
                    pal::RED
                } else if active {
                    pal::CYAN
                } else {
                    Color::rgb(60, 70, 120)
                };
                a.border(rect, rim, 1.0);
            }
        }
    }
    ui.draw(r, screen, &env);
    if let Some(s) = session {
        let m = &s.m;
        let side = s.human.unwrap_or(0);
        // The radar: everyone and the ball, top down.
        if let Some((mut a, size)) = art(r, &ui, screen, "radar", NO_ITEM) {
            let map = Rect::new(0.0, 0.0, size.x, size.y);
            let (mx, my) = (size.x * 0.5, size.y * 0.5);
            a.fill(map, Color::rgb(36, 110, 50));
            a.border(map, Color::rgb(220, 235, 220), 1.0);
            a.fill(Rect::new(mx, 0.0, 1.0, size.y), Color::rgb(220, 235, 220));
            for i in 0..N {
                let p = &m.players[i];
                if p.off {
                    continue;
                }
                let x = mx + p.pos.x.to_f32() / 52.5 * (mx - 0.0);
                let y = my + p.pos.y.to_f32() / 34.0 * (my - 0.0);
                a.fill(
                    Rect::new(x - 2.0, y - 2.0, 4.0, 4.0),
                    s.colors.shirt[side_of(i)],
                );
            }
            let bx = mx + m.ball.pos.x.to_f32() / 52.5 * mx;
            let by = my + m.ball.pos.z.to_f32() / 34.0 * my;
            a.fill(Rect::new(bx - 1.5, by - 1.5, 3.0, 3.0), Color::WHITE);
        }
        for k in 0..SIDE {
            if let Some((mut a, size)) = art(r, &ui, screen, BARS[k], NO_ITEM) {
                let st = m.players[side * SIDE + k].stamina.to_f32();
                let c = if st > 0.6 {
                    pal::GREEN
                } else if st > 0.35 {
                    pal::YELLOW
                } else {
                    pal::RED
                };
                a.bar(Rect::new(0.0, 0.0, size.x, size.y), st, c);
            }
            if let Some((on, rect)) = ui.rect_of_item(screen, TILES[k], NO_ITEM) {
                g.hits.add(rect, on, k as u8);
            }
        }
        if let Some((on, rect)) = ui.rect_of_item(screen, "formation", NO_ITEM) {
            g.hits.add(rect, on, 100);
        }
    }
    g.bottom_ui = ui;
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
