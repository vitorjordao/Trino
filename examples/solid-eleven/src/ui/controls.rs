//! The settings' controls page (`ui/controls.ui.ron`, issue #95 and issue #86
//! item 7): a tab for the match's buttons (every target) and, on the PC,
//! tabs for the keyboard's keys and the gamepad's buttons. The list on the
//! left changes a binding; the right draws the device itself with each
//! action on its button, so the layout reads at a glance — the N64 pad, the
//! 3DS, a PC gamepad (in its family's chassis and glyphs) or a keyboard.

use core::fmt::Write as _;

use crate::game::app::{CONTROL_ROWS_SHOWN, Device, InputPeek, SolidElevenGame};
use crate::game::input::{ACTIONS, Action, Bindings};
use crate::ui::i18n::{Lang, S, tr};
use crate::ui::scr::{art, flag, int, rev, show, txt};
use crate::ui::{Size, Ui, pal};
use trino_core::ui::{NO_ITEM, UiValue};
use trino_core::{BindDevice, Button, Color, PadFamily, Rect, Renderer, TextAlign, TextBuf, Vec2};

const DIGITS: [&str; ACTIONS] = ["1", "2", "3", "4", "5", "6", "7"];

/// An action's long name (the list) and short one (the diagram).
pub fn action_names(a: Action) -> (S, S) {
    match a {
        Action::Pass => (S::ActPass, S::ShortPass),
        Action::Shoot => (S::ActShoot, S::ShortShoot),
        Action::Through => (S::ActThrough, S::ShortThrough),
        Action::Keeper => (S::ActKeeper, S::ShortKeeper),
        Action::Switch => (S::ActSwitch, S::ShortSwitch),
        Action::Sprint => (S::ActSprint, S::ShortSprint),
        Action::Pause => (S::ActPause, S::ShortPause),
    }
}

/// Each action's color and number: the list's badge and the badge drawn
/// on its button in the diagram.
pub fn action_color(a: Action) -> Color {
    match a {
        Action::Pass => pal::GREEN,
        Action::Shoot => pal::RED,
        Action::Through => pal::BLUE,
        Action::Keeper => pal::YELLOW,
        Action::Switch => pal::ORANGE,
        Action::Sprint => pal::PURPLE,
        Action::Pause => pal::INK,
    }
}

fn badge(ui: &mut Ui, x: f32, y: f32, a: Action) {
    ui.fill(Rect::new(x, y, 9.0, 9.0), action_color(a));
    // At the top: the PC's big font inks a pixel lower than the 11 px one.
    ui.text(
        x + 4.5,
        y,
        DIGITS[a as usize],
        pal::NAVY,
        Size::Small,
        TextAlign::Center,
    );
}

/// The action on engine button `b`, if any.
pub fn action_on(bind: &Bindings, b: Button) -> Option<Action> {
    Action::ALL.into_iter().find(|&a| bind.of(a) == b)
}

/// A button's name as printed on `device` (`None`: the controller has no
/// such button — the N64 has no L2/R2/L3/R3, the 3DS no L3/R3).
pub fn button_name(device: Device, b: Button) -> Option<&'static str> {
    use Button::*;
    Some(match (device, b) {
        (_, A) => "A",
        (_, B) => "B",
        (Device::N64, X) => "C-L",
        (Device::N64, Y) => "C-D",
        (_, X) => "X",
        (_, Y) => "Y",
        (Device::Pc, L) => "LB",
        (Device::Pc, R) => "RB",
        (_, L) => "L",
        (_, R) => "R",
        (Device::Pc, L2) => "LT",
        (Device::Pc, R2) => "RT",
        (Device::N3ds, L2) => "ZL",
        (Device::N3ds, R2) => "ZR",
        (Device::Pc, L3) => "LS",
        (Device::Pc, R3) => "RS",
        (Device::N64, L2 | R2 | L3 | R3) | (Device::N3ds, L3 | R3) => return None,
        (_, Start) => "START",
        (Device::N64, Select) => "Z",
        (Device::Pc, Select) => "BACK",
        (_, Select) => "SELECT",
        (_, DpadUp) => "UP",
        (_, DpadDown) => "DOWN",
        (_, DpadLeft) => "LEFT",
        (_, DpadRight) => "RIGHT",
    })
}

/// A button on a diagram: its printed name and where it sits (design
/// units, relative to the diagram's top-left), and which side its label
/// goes.
#[derive(Clone, Copy)]
struct Spot {
    name: &'static str,
    at: (f32, f32),
    right: bool,
}

const fn spot(name: &'static str, x: f32, y: f32, right: bool) -> Spot {
    Spot {
        name,
        at: (x, y),
        right,
    }
}

/// An Xbox-style PC gamepad (the diagram is about 144 x 92 units).
const PAD: [Spot; 12] = [
    spot("LT", 24.0, 4.0, false),
    spot("LB", 24.0, 13.0, false),
    spot("RT", 120.0, 4.0, true),
    spot("RB", 120.0, 13.0, true),
    spot("BACK", 52.0, 30.0, false),
    spot("START", 82.0, 30.0, true),
    spot("LS", 30.0, 41.0, false),
    spot("RS", 96.0, 61.0, true),
    spot("Y", 118.0, 30.0, true),
    spot("X", 106.0, 40.0, false),
    spot("B", 130.0, 40.0, true),
    spot("A", 118.0, 50.0, true),
];

/// The N64 pad (C-left and C-down are X and Y; Z, under the middle
/// prong, is Select).
const N64: [Spot; 8] = [
    spot("L", 18.0, 10.0, false),
    spot("R", 126.0, 10.0, true),
    spot("START", 72.0, 32.0, false),
    spot("Z", 72.0, 84.0, false),
    spot("C-L", 112.0, 30.0, false),
    spot("C-D", 126.0, 40.0, true),
    spot("B", 98.0, 44.0, false),
    spot("A", 110.0, 54.0, true),
];

/// The 3DS.
const N3DS: [Spot; 10] = [
    spot("L", 12.0, 6.0, false),
    spot("ZL", 34.0, 6.0, false),
    spot("ZR", 110.0, 6.0, true),
    spot("R", 132.0, 6.0, true),
    spot("X", 118.0, 26.0, true),
    spot("Y", 108.0, 36.0, false),
    spot("A", 128.0, 36.0, true),
    spot("B", 118.0, 46.0, true),
    spot("SELECT", 52.0, 76.0, false),
    spot("START", 92.0, 76.0, true),
];

/// The keyboard keys drawn (the host's names), by row; WASD and the
/// arrows move. Keys outside it are listed under the drawing.
const KEY_ROWS: [&[&str]; 5] = [
    &["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"],
    &["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"],
    &["A", "S", "D", "F", "G", "H", "J", "K", "L", ";"],
    &["LSHIFT", "Z", "X", "C", "V", "B", "N", "M", ",", "."],
    &["LALT", "SPACE", "ENTER", "RSHIFT"],
];

fn status_of(g: &SolidElevenGame) -> Option<(&str, Color)> {
    let lang = g.settings.lang;
    let bind = g.settings.bindings;
    if g.capture.is_some() {
        Some((tr(lang, S::PressButton), pal::LIME))
    } else if let Some((_, d)) = g.peek.capturing {
        Some((
            tr(
                lang,
                if d == BindDevice::Keyboard {
                    S::PressKey
                } else {
                    S::PressPad
                },
            ),
            pal::LIME,
        ))
    } else if g.cur.ctl_tab == 2 && g.peek.gamepad != 2 {
        Some((
            tr(
                lang,
                if g.peek.gamepad == 0 {
                    S::PadUnsupported
                } else {
                    S::NoPad
                },
            ),
            pal::ORANGE,
        ))
    } else if g.cur.ctl_tab == 2 && !g.peek.pad_name.as_str().is_empty() {
        Some((g.peek.pad_name.as_str(), pal::DIM))
    } else if g.cur.ctl_tab == 0
        && Action::ALL
            .iter()
            .any(|&a| button_name(g.device, bind.of(a)).is_none())
    {
        Some((tr(lang, S::NotOnPad), pal::RED))
    } else {
        None
    }
}

/// The controls screen (`ui/controls.ui.ron`).
pub fn draw(g: &mut SolidElevenGame, r: &mut dyn Renderer) {
    const TABS: [&str; 3] = ["tab0", "tab1", "tab2"];
    let screen = &super::controls_gen::SCREEN;
    let big = g.access.clamped().text_factor() >= 1.5;
    let tabs = g.control_tabs();
    let rows = g.control_rows();
    let shown = rows.min(if big { 3 } else { CONTROL_ROWS_SHOWN });
    let top = g
        .cur
        .ctl_row
        .saturating_sub(shown - 1)
        .max(g.cur.ctl_top.min(g.cur.ctl_row))
        .min(rows.saturating_sub(shown));
    let row_id = if big { "row_big" } else { "row" };
    let badge_id = if big { "badge_big" } else { "badge" };
    let pill_id = if big { "pill_big" } else { "pill" };
    let status_id = if big { "status_big" } else { "status_box" };
    let state = rev(&(
        g.cur.ctl_tab,
        top,
        g.device as u8,
        g.settings.bindings.to_bytes(),
        g.peek.rebindable,
        g.peek.gamepad,
        g.peek.pad_family,
        g.peek.pad_name.as_str(),
        g.peek.capturing.map(|(b, d)| (b as u8, d as u8)),
        g.capture.map(|c| c.action as u8),
    ));
    show(
        g,
        r,
        screen,
        Some((row_id, (g.cur.ctl_row - top) as u16)),
        state,
        |g, key, item| value(g, top, shown, rows, tabs, key, item),
        |g, ui, r| {
            super::scr::chrome(ui, screen, r);
            let tab_id = TABS[g.cur.ctl_tab as usize % 3];
            if let Some((mut a, size)) = art(r, ui, screen, tab_id, NO_ITEM) {
                let rect = Rect::new(0.0, 0.0, size.x, size.y);
                a.fill(rect, Color::rgba(30, 60, 130, 240));
                a.border(rect, pal::CYAN, 1.0);
            }
            for item in 0..shown {
                let k = top + item;
                if k + 1 >= rows {
                    continue;
                }
                let act = if g.cur.ctl_tab == 0 {
                    Some(Action::ALL[k])
                } else {
                    action_on(&g.settings.bindings, Button::ALL[k])
                };
                if let Some(act) = act
                    && let Some((mut a, size)) = art(r, ui, screen, badge_id, item as u16)
                {
                    a.fill(Rect::new(0.0, 0.0, size.x, size.y), action_color(act));
                }
                if let Some((mut a, size)) = art(r, ui, screen, pill_id, item as u16) {
                    let c = if g.cur.ctl_tab == 0 {
                        let b = g.settings.bindings.of(Action::ALL[k]);
                        if button_name(g.device, b).is_some() {
                            pal::CYAN
                        } else {
                            pal::RED
                        }
                    } else {
                        pal::YELLOW
                    };
                    a.fill(Rect::new(0.0, 0.0, size.x, size.y), c);
                }
            }
            if let Some((_, c)) = status_of(g)
                && let Some((mut a, size)) = art(r, ui, screen, status_id, NO_ITEM)
            {
                a.border(Rect::new(0.0, 0.0, size.x, size.y), c, 1.0);
                if let Some(cap) = g.capture {
                    let f = (cap.left / crate::game::app::CAPTURE_SECS).clamp(0.0, 1.0);
                    a.fill(Rect::new(2.0, size.y - 2.0, (size.x - 4.0) * f, 2.0), c);
                }
            }
            if let Some((mut a, size)) = art(r, ui, screen, "diagram", NO_ITEM) {
                a.border(
                    Rect::new(0.0, 0.0, size.x, size.y),
                    Color::rgb(60, 70, 120),
                    1.0,
                );
            }
        },
    );
    let ui = core::mem::take(&mut g.menu_ui);
    if let Some((mut a, size)) = art(r, &ui, screen, "diagram", NO_ITEM) {
        let at = Vec2::new(((size.x - 148.0) * 0.5).max(0.0), 2.0);
        let bind = g.settings.bindings;
        let lang = g.settings.lang;
        match (g.cur.ctl_tab, g.device) {
            (1, _) => keyboard(&mut a, at, &g.peek, &bind, lang),
            (0, Device::N64) => diagram(&mut a, at, &N64, Device::N64, &bind, None, lang),
            (0, Device::N3ds) => diagram(&mut a, at, &N3DS, Device::N3ds, &bind, None, lang),
            _ => match g.peek.pad_family {
                Some(fam) => family_diagram(&mut a, at, fam, &bind, Some(&g.peek)),
                None => diagram(&mut a, at, &PAD, Device::Pc, &bind, Some(&g.peek), lang),
            },
        }
    }
    g.menu_ui = ui;
    for (t, id) in TABS.iter().enumerate().take(tabs as usize) {
        super::scr::hit(g, screen, id, NO_ITEM, 200 + t as u8);
    }
    for k in 0..shown {
        super::scr::hit(g, screen, row_id, k as u16, (top + k) as u8);
    }
    super::scr::back_hits(g, screen);
}

fn value(
    g: &SolidElevenGame,
    top: usize,
    shown: usize,
    rows: usize,
    tabs: u8,
    key: &str,
    item: u16,
) -> Option<UiValue> {
    let lang = g.settings.lang;
    let bind = g.settings.bindings;
    match key {
        "pc_tabs" => return flag(tabs > 1),
        "tabs_hint" => {
            let switch_hint = if !g.using_pad && g.device == Device::Pc {
                "Q / E"
            } else {
                "L / R"
            };
            let mut b = TextBuf::<48>::new();
            let _ = write!(b, "{}: {}", switch_hint, tr(lang, S::TabsHint));
            return txt(b.as_str());
        }
        "rows" => return int(shown as i32),
        "move_hint" => {
            let mv = if g.device == Device::Pc && g.cur.ctl_tab == 1 {
                S::MoveKeys
            } else {
                S::MoveStick
            };
            return txt(tr(lang, mv));
        }
        "has_status" => return flag(status_of(g).is_some()),
        "status" => return txt(status_of(g).map_or("", |(s, _)| s)),
        _ => {}
    }
    let k = top + item as usize;
    if k >= rows {
        return None;
    }
    match key {
        "is_reset" => return flag(k + 1 == rows),
        "is_bind" => return flag(k + 1 < rows),
        _ if k + 1 == rows => return None,
        _ => {}
    }
    if g.cur.ctl_tab == 0 {
        let a = Action::ALL[k];
        match key {
            "has_badge" => flag(true),
            "badge_num" => txt(DIGITS[a as usize]),
            "row_left" => txt(tr(lang, action_names(a).0)),
            "row_right" => {
                let b = bind.of(a);
                let label = if g.device == Device::Pc
                    && let Some(fam) = g.peek.pad_family
                {
                    Some(super::glyphs::label(fam, b))
                } else {
                    button_name(g.device, b)
                };
                txt(label.unwrap_or("?"))
            }
            _ => None,
        }
    } else {
        let b = Button::ALL[k];
        let act = action_on(&bind, b);
        match key {
            "has_badge" => flag(act.is_some()),
            "badge_num" => txt(act.map_or("", |a| DIGITS[a as usize])),
            "row_left" => {
                let name = if g.cur.ctl_tab == 2
                    && let Some(fam) = g.peek.pad_family
                {
                    super::glyphs::label(fam, b)
                } else {
                    button_name(Device::Pc, b).unwrap_or("?")
                };
                let mut t = TextBuf::<48>::new();
                match act {
                    Some(a) => {
                        let _ = write!(t, "{}  {}", name, tr(lang, action_names(a).1));
                    }
                    None => {
                        let _ = write!(t, "{name}  -");
                    }
                }
                txt(t.as_str())
            }
            "row_right" => {
                let keys = if g.cur.ctl_tab == 1 {
                    &g.peek.keys
                } else {
                    &g.peek.pads
                };
                let s = keys[k].as_str();
                txt(if s.is_empty() { "-" } else { s })
            }
            _ => None,
        }
    }
}

/// The action on a printed button: the console's fixed buttons, or the
/// PC pad's physical buttons as the host binds them (`peek`).
fn action_at(
    name: &str,
    device: Device,
    bind: &Bindings,
    peek: Option<&InputPeek>,
) -> Option<Action> {
    // A control that presses two buttons (LT: "LB/LT" is L, "LT" is L2)
    // shows the specific one's action first, as the match reads it.
    let mut alias = None;
    for (k, b) in Button::ALL.into_iter().enumerate() {
        let (on, exact) = match peek {
            Some(p) if p.rebindable => {
                let names = p.pads[k].as_str();
                (names.split('/').any(|n| n == name), names == name)
            }
            _ => (button_name(device, b) == Some(name), true),
        };
        if let (true, Some(a)) = (on, action_on(bind, b)) {
            if exact {
                return Some(a);
            }
            alias = alias.or(Some(a));
        }
    }
    alias
}

/// A PC/Web gamepad drawn from its [`PadFamily`]'s [`super::glyphs::Chassis`]
/// and [`super::glyphs::glyph`] table (PlayStation symbols, Nintendo swapped
/// face letters, 8BitDo SN30 body, Xbox/Generic).
fn family_diagram(
    ui: &mut Ui,
    at: Vec2,
    family: PadFamily,
    bind: &Bindings,
    peek: Option<&InputPeek>,
) {
    const BUTTONS: [Button; 12] = [
        Button::L2,
        Button::L,
        Button::R2,
        Button::R,
        Button::Select,
        Button::Start,
        Button::L3,
        Button::R3,
        Button::Y,
        Button::X,
        Button::B,
        Button::A,
    ];
    let o = Vec2::new(at.x + 2.0, at.y + 6.0);
    let (w_box, h_box) = (136.0, 84.0);
    let body = Color::rgb(70, 78, 110);
    let dark = Color::rgb(40, 46, 72);
    let ch = super::glyphs::chassis(family);
    if family == PadFamily::EightBitDo {
        ui.fill(Rect::new(o.x + 8.0, o.y + 18.0, 128.0, 62.0), body);
    } else {
        ui.polygon(
            &[
                Vec2::new(o.x + 14.0, o.y + 20.0),
                Vec2::new(o.x + 130.0, o.y + 20.0),
                Vec2::new(o.x + 144.0, o.y + 54.0),
                Vec2::new(o.x + 132.0, o.y + 84.0),
                Vec2::new(o.x + 112.0, o.y + 74.0),
                Vec2::new(o.x + 32.0, o.y + 74.0),
                Vec2::new(o.x + 12.0, o.y + 84.0),
                Vec2::new(o.x, o.y + 54.0),
            ],
            body,
        );
    }
    let (dx, dy) = (o.x + ch.dpad.0 * w_box, o.y + ch.dpad.1 * h_box);
    ui.fill(Rect::new(dx - 2.0, dy - 7.0, 4.0, 14.0), dark);
    ui.fill(Rect::new(dx - 7.0, dy - 2.0, 14.0, 4.0), dark);
    for (sx, sy) in [ch.left_stick, ch.right_stick] {
        ui.fill(
            Rect::new(o.x + sx * w_box - 6.5, o.y + sy * h_box - 6.5, 13.0, 13.0),
            dark,
        );
    }
    let _ = ch.home;
    for b in BUTTONS {
        let (nx, ny) = super::glyphs::spot(family, b);
        let p = Vec2::new(o.x + nx * w_box, o.y + ny * h_box + 4.0);
        let gl = super::glyphs::glyph(family, b);
        let xbox_name = button_name(Device::Pc, b).unwrap_or("?");
        let action = action_at(xbox_name, Device::Pc, bind, peek);
        let right = nx >= 0.5;
        let w = if gl.shape.is_symbol() {
            9.0
        } else {
            let _ = (gl.name, gl.fill);
            let w = ui.measure(gl.label, Size::Small) + 4.0;
            let c = if action.is_some() {
                Color::rgb(210, 216, 236)
            } else {
                Color::rgb(120, 128, 160)
            };
            ui.fill(Rect::new(p.x - w * 0.5, p.y - 4.0, w, 9.0), c);
            ui.text(
                p.x,
                p.y - 4.0,
                gl.label,
                pal::NAVY,
                Size::Small,
                TextAlign::Center,
            );
            w
        };
        if gl.shape.is_symbol() {
            ui.fill(Rect::new(p.x - 4.5, p.y - 4.0, 9.0, 9.0), gl.fill);
            for row in 0..7u8 {
                for (x0, x1) in super::glyphs::spans(gl.shape, 7, row).into_iter().flatten() {
                    ui.fill(
                        Rect::new(
                            p.x - 3.5 + x0 as f32,
                            p.y - 3.0 + row as f32,
                            (x1 - x0) as f32,
                            1.0,
                        ),
                        gl.ink,
                    );
                }
            }
        }
        if let Some(a) = action {
            let bx = if right {
                p.x + w * 0.5 + 1.0
            } else {
                p.x - w * 0.5 - 10.0
            };
            badge(ui, bx, p.y - 4.0, a);
        }
    }
}

/// A controller drawn as a few shapes, every button as a small cap with
/// its printed name, and each assigned action's numbered badge on its
/// button (the list's badges match).
fn diagram(
    ui: &mut Ui,
    at: Vec2,
    spots: &[Spot],
    device: Device,
    bind: &Bindings,
    peek: Option<&InputPeek>,
    _lang: Lang,
) {
    let o = Vec2::new(at.x + 2.0, at.y + 6.0);
    let body = Color::rgb(70, 78, 110);
    let dark = Color::rgb(40, 46, 72);
    match device {
        Device::N64 => {
            // Three prongs and the grey body.
            ui.polygon(
                &[
                    Vec2::new(o.x + 6.0, o.y + 18.0),
                    Vec2::new(o.x + 138.0, o.y + 18.0),
                    Vec2::new(o.x + 136.0, o.y + 52.0),
                    Vec2::new(o.x + 122.0, o.y + 80.0),
                    Vec2::new(o.x + 100.0, o.y + 56.0),
                    Vec2::new(o.x + 82.0, o.y + 92.0),
                    Vec2::new(o.x + 62.0, o.y + 92.0),
                    Vec2::new(o.x + 44.0, o.y + 56.0),
                    Vec2::new(o.x + 22.0, o.y + 80.0),
                    Vec2::new(o.x + 8.0, o.y + 52.0),
                ],
                body,
            );
            // The stick in the middle prong, the D-pad on the left.
            ui.fill(Rect::new(o.x + 66.0, o.y + 58.0, 12.0, 12.0), dark);
            ui.fill(Rect::new(o.x + 26.0, o.y + 26.0, 4.0, 14.0), dark);
            ui.fill(Rect::new(o.x + 21.0, o.y + 31.0, 14.0, 4.0), dark);
        }
        Device::N3ds => {
            ui.fill(Rect::new(o.x + 4.0, o.y + 14.0, 136.0, 72.0), body);
            ui.fill(
                Rect::new(o.x + 42.0, o.y + 20.0, 60.0, 44.0),
                Color::rgb(30, 34, 52),
            );
            ui.fill(Rect::new(o.x + 14.0, o.y + 22.0, 13.0, 13.0), dark);
            ui.fill(Rect::new(o.x + 18.5, o.y + 42.0, 4.0, 14.0), dark);
            ui.fill(Rect::new(o.x + 13.5, o.y + 47.0, 14.0, 4.0), dark);
        }
        Device::Pc => {
            ui.polygon(
                &[
                    Vec2::new(o.x + 14.0, o.y + 20.0),
                    Vec2::new(o.x + 130.0, o.y + 20.0),
                    Vec2::new(o.x + 144.0, o.y + 54.0),
                    Vec2::new(o.x + 132.0, o.y + 84.0),
                    Vec2::new(o.x + 112.0, o.y + 74.0),
                    Vec2::new(o.x + 32.0, o.y + 74.0),
                    Vec2::new(o.x + 12.0, o.y + 84.0),
                    Vec2::new(o.x, o.y + 54.0),
                ],
                body,
            );
            // Sticks and D-pad.
            ui.fill(Rect::new(o.x + 24.0, o.y + 34.0, 13.0, 13.0), dark);
            ui.fill(Rect::new(o.x + 90.0, o.y + 54.0, 13.0, 13.0), dark);
            ui.fill(Rect::new(o.x + 50.0, o.y + 50.0, 4.0, 14.0), dark);
            ui.fill(Rect::new(o.x + 45.0, o.y + 55.0, 14.0, 4.0), dark);
        }
    }
    for s in spots {
        let p = Vec2::new(o.x + s.at.0, o.y + s.at.1);
        let w = ui.measure(s.name, Size::Small) + 4.0;
        let action = action_at(s.name, device, bind, peek);
        let c = if action.is_some() {
            Color::rgb(210, 216, 236)
        } else {
            Color::rgb(120, 128, 160)
        };
        // The cap with its printed name; the action's badge beside it,
        // outward.
        ui.fill(Rect::new(p.x - w * 0.5, p.y - 4.0, w, 9.0), c);
        ui.text(
            p.x,
            p.y - 4.0,
            s.name,
            pal::NAVY,
            Size::Small,
            TextAlign::Center,
        );
        if let Some(a) = action {
            let bx = if s.right && p.x + w * 0.5 + 10.0 <= o.x + 144.0 {
                p.x + w * 0.5 + 1.0
            } else {
                p.x - w * 0.5 - 10.0
            };
            badge(ui, bx, p.y - 4.0, a);
        }
    }
}

/// A keyboard: its letter rows as key caps, the bound keys lit with their
/// action's short name under the cap, the rest listed below.
fn keyboard(ui: &mut Ui, at: Vec2, peek: &InputPeek, bind: &Bindings, lang: Lang) {
    let action_of_key = |key: &str| -> Option<Action> {
        Button::ALL
            .iter()
            .enumerate()
            .find(|(k, _)| peek.keys[*k].as_str() == key)
            .and_then(|(_, b)| action_on(bind, *b))
    };
    let cap = |ui: &mut Ui, x: f32, y: f32, w: f32, key: &str| {
        let moves = matches!(
            key,
            "W" | "A" | "S" | "D" | "UP" | "DOWN" | "LEFT" | "RIGHT"
        );
        let c = match action_of_key(key) {
            Some(a) => action_color(a),
            None if moves => pal::LIME,
            None => Color::rgb(70, 78, 110),
        };
        ui.fill(Rect::new(x, y, w - 1.0, 10.0), c);
        // The host's names, as printed on a keyboard.
        let label = match key {
            "LSHIFT" | "RSHIFT" => "SHIFT",
            "LALT" => "ALT",
            "RALT" => "ALTGR",
            "UP" | "DOWN" | "LEFT" | "RIGHT" => "",
            k => k,
        };
        ui.text(
            x + (w - 1.0) * 0.5,
            y + 1.0,
            label,
            pal::NAVY,
            Size::Small,
            TextAlign::Center,
        );
    };
    let mut y = at.y + 2.0;
    for (row, keys) in KEY_ROWS.iter().enumerate() {
        let mut x = at.x + [0.0, 0.0, 4.0, 0.0, 0.0][row];
        for key in keys.iter() {
            let w = match *key {
                "SPACE" => 36.0,
                k if k.len() > 2 => 27.0,
                _ => 11.5,
            };
            cap(ui, x, y, w, key);
            x += w;
        }
        y += 11.5;
    }
    // The arrows (they move too; bindable as the D-pad): up beside the
    // bottom row, the others under it.
    let ax = at.x + 118.0;
    for (key, dx, dy) in [
        ("UP", 11.5, -11.5),
        ("LEFT", 0.0, 0.0),
        ("DOWN", 11.5, 0.0),
        ("RIGHT", 23.0, 0.0),
    ] {
        cap(ui, ax + dx, y + dy, 11.5, key);
    }
    // Every action's key (the drawing's legend, and the keys it leaves
    // out).
    y += 13.0;
    let mut x = at.x;
    for a in Action::ALL {
        let b = bind.of(a);
        let k = Button::ALL.iter().position(|x| *x == b).unwrap_or(0);
        let key = peek.keys[k].as_str();
        let key = if key.is_empty() { "-" } else { key };
        let mut t = TextBuf::<28>::new();
        let _ = write!(t, "{} {}", key, tr(lang, action_names(a).1));
        let w = ui.measure(t.as_str(), Size::Small) + 16.0;
        if x + w > at.x + 148.0 {
            x = at.x;
            y += 11.0;
        }
        badge(ui, x, y, a);
        ui.text(
            x + 11.0,
            y,
            t.as_str(),
            pal::INK,
            Size::Small,
            TextAlign::Left,
        );
        x += w;
    }
}
