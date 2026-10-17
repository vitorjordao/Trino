//! The settings' controls page: a tab for the match's buttons (every
//! target) and, on the PC, tabs for the keyboard's keys and the gamepad's
//! buttons. The list on the left changes a binding; the right draws the
//! device itself with each action on its button, so the layout reads at a
//! glance — the N64 pad, the 3DS, a PC gamepad or a keyboard.

use core::fmt::Write as _;

use crate::game::app::{CONTROL_ROWS_SHOWN, Device, InputPeek, SolidElevenGame};
use crate::game::input::{ACTIONS, Action, Bindings};
use crate::ui::i18n::{Lang, S, tr};
use crate::ui::{Size, Ui, pal};
use trino_core::{BindDevice, Button, Color, Rect, Screen, TextAlign, TextBuf, Vec2};

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
    const DIGITS: [&str; ACTIONS] = ["1", "2", "3", "4", "5", "6", "7"];
    ui.fill(Rect::new(x, y, 9.0, 9.0), action_color(a));
    ui.text(
        x + 4.5,
        y + 0.5,
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
const KEY_ROWS: [&[&str]; 4] = [
    &["Q", "W", "E", "R", "T", "Y", "U", "I", "O", "P"],
    &["A", "S", "D", "F", "G", "H", "J", "K", "L", ";"],
    &["LSHIFT", "Z", "X", "C", "V", "B", "N", "M", ",", "."],
    &["LALT", "SPACE", "ENTER", "RSHIFT"],
];

fn panel_row(ui: &mut Ui, r: Rect, sel: bool, rim: Color) {
    ui.fill(
        r,
        if sel {
            Color::rgba(30, 60, 130, 235)
        } else {
            pal::PANEL
        },
    );
    ui.border(r, if sel { rim } else { Color::rgb(50, 60, 100) }, 1.0);
}

/// A small key cap / button pill with `text`.
fn pill(ui: &mut Ui, x: f32, y: f32, text: &str, c: Color) -> f32 {
    let w = ui.measure(text, Size::Small) + 6.0;
    ui.fill(Rect::new(x, y, w, 10.0), c);
    ui.text(
        x + w * 0.5,
        y + 1.0,
        text,
        pal::NAVY,
        Size::Small,
        TextAlign::Center,
    );
    w
}

/// The page.
pub fn draw(g: &mut SolidElevenGame, ui: &mut Ui) {
    let lang = g.settings.lang;
    let tabs = g.control_tabs();
    let device = g.device;
    // Tabs.
    let names = [S::TabMatch, S::TabKeyboard, S::TabPad];
    let mut x = 8.0;
    for t in 0..tabs {
        let label = tr(lang, names[t as usize]);
        let w = ui.measure(label, Size::Small) + 16.0;
        let r = Rect::new(x, 32.0, w, 13.0);
        let sel = g.cur.ctl_tab == t;
        ui.slant(
            r,
            if sel {
                Color::rgba(30, 60, 130, 240)
            } else {
                pal::PANEL
            },
            3.0,
        );
        if sel {
            ui.slant_border(r, pal::CYAN, 3.0, 1.0);
        }
        ui.text(
            x + w * 0.5,
            34.0,
            label,
            pal::INK,
            Size::Small,
            TextAlign::Center,
        );
        let hit = ui.rect(r);
        g.hits.add(hit, Screen::Top, 200 + t);
        x += w + 4.0;
    }
    if tabs > 1 {
        let mut b = TextBuf::<24>::new();
        let _ = write!(b, "L / R: {}", tr(lang, S::TabsHint));
        ui.text(
            312.0,
            34.0,
            b.as_str(),
            pal::DIM,
            Size::Small,
            TextAlign::Right,
        );
    }
    // The list.
    let bind = g.settings.bindings;
    let rows = g.control_rows();
    let top = g.cur.ctl_top;
    for k in top..rows.min(top + CONTROL_ROWS_SHOWN) {
        let y = 50.0 + (k - top) as f32 * 17.0;
        let r = Rect::new(6.0, y, 150.0, 15.0);
        let sel = g.cur.ctl_row == k;
        panel_row(ui, r, sel, pal::CYAN);
        let hit = ui.rect(r);
        g.hits.add(hit, Screen::Top, k as u8);
        if k + 1 == rows {
            ui.text(
                81.0,
                y + 3.0,
                tr(lang, S::ResetDefaults),
                pal::YELLOW,
                Size::Small,
                TextAlign::Center,
            );
            continue;
        }
        if g.cur.ctl_tab == 0 {
            let a = Action::ALL[k];
            badge(ui, 9.0, y + 3.0, a);
            ui.text(
                21.0,
                y + 3.0,
                tr(lang, action_names(a).0),
                pal::INK,
                Size::Small,
                TextAlign::Left,
            );
            let b = bind.of(a);
            let (label, c) = match button_name(device, b) {
                Some(n) => (n, pal::CYAN),
                None => ("?", pal::RED),
            };
            let w = ui.measure(label, Size::Small) + 6.0;
            pill(ui, 152.0 - w, y + 2.5, label, c);
        } else {
            let b = Button::ALL[k];
            let keys = if g.cur.ctl_tab == 1 {
                &g.peek.keys
            } else {
                &g.peek.pads
            };
            let name = button_name(Device::Pc, b).unwrap_or("?");
            ui.text(10.0, y + 3.0, name, pal::DIM, Size::Small, TextAlign::Left);
            match action_on(&bind, b) {
                Some(a) => {
                    badge(ui, 44.0, y + 3.0, a);
                    ui.text(
                        56.0,
                        y + 3.0,
                        tr(lang, action_names(a).1),
                        pal::INK,
                        Size::Small,
                        TextAlign::Left,
                    );
                }
                None => ui.text(56.0, y + 3.0, "-", pal::DIM, Size::Small, TextAlign::Left),
            }
            let key = keys[k].as_str();
            let label = if key.is_empty() { "-" } else { key };
            let w = ui.measure(label, Size::Small) + 6.0;
            pill(ui, 152.0 - w, y + 2.5, label, pal::YELLOW);
        }
    }
    // The device, with each action on its button.
    let at = Vec2::new(164.0, 50.0);
    ui.panel(
        Rect::new(at.x - 4.0, at.y - 2.0, 156.0, 120.0),
        Color::rgb(60, 70, 120),
    );
    match (g.cur.ctl_tab, device) {
        (1, _) => keyboard(ui, at, &g.peek, &bind, lang),
        (0, Device::N64) => diagram(ui, at, &N64, Device::N64, &bind, None, lang),
        (0, Device::N3ds) => diagram(ui, at, &N3DS, Device::N3ds, &bind, None, lang),
        _ => diagram(ui, at, &PAD, Device::Pc, &bind, Some(&g.peek), lang),
    }
    // What moves the player, and the state line.
    let mv = if device == Device::Pc && g.cur.ctl_tab == 1 {
        S::MoveKeys
    } else {
        S::MoveStick
    };
    ui.text(
        238.0,
        158.0,
        tr(lang, mv),
        pal::DIM,
        Size::Small,
        TextAlign::Center,
    );
    let status: Option<(&str, Color)> = if g.capture.is_some() {
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
            .any(|&a| button_name(device, bind.of(a)).is_none())
    {
        Some((tr(lang, S::NotOnPad), pal::RED))
    } else {
        None
    };
    if let Some((text, c)) = status {
        let w = ui.measure(text, Size::Small) + 16.0;
        let r = Rect::new(238.0 - w * 0.5, 172.0, w, 14.0);
        ui.slant(r, Color::rgba(10, 12, 30, 235), 3.0);
        ui.slant_border(r, c, 3.0, 1.0);
        ui.text(238.0, 174.0, text, c, Size::Small, TextAlign::Center);
        if let Some(cap) = g.capture {
            // The time left to press it.
            let k = (cap.left / crate::game::app::CAPTURE_SECS).clamp(0.0, 1.0);
            ui.fill(Rect::new(r.pos.x + 2.0, 184.0, (w - 4.0) * k, 2.0), c);
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
    for (k, b) in Button::ALL.into_iter().enumerate() {
        let on = match peek {
            Some(p) if p.rebindable => p.pads[k].as_str().split('/').any(|n| n == name),
            _ => button_name(device, b) == Some(name),
        };
        if on && let Some(a) = action_on(bind, b) {
            return Some(a);
        }
    }
    None
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
            p.y - 3.5,
            s.name,
            pal::NAVY,
            Size::Small,
            TextAlign::Center,
        );
        if let Some(a) = action {
            let bx = if s.right {
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
        ui.fill(Rect::new(x, y, w - 1.0, 11.0), c);
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
            y + 1.5,
            label,
            pal::NAVY,
            Size::Small,
            TextAlign::Center,
        );
    };
    let mut y = at.y + 2.0;
    for (row, keys) in KEY_ROWS.iter().enumerate() {
        let mut x = at.x + [0.0, 4.0, 0.0, 0.0][row];
        for key in keys.iter() {
            let w = match *key {
                "SPACE" => 36.0,
                k if k.len() > 2 => 27.0,
                _ => 11.5,
            };
            cap(ui, x, y, w, key);
            x += w;
        }
        y += 13.0;
    }
    // The arrows (they move too; bindable as the D-pad): up beside the
    // bottom row, the others under it.
    let ax = at.x + 118.0;
    for (key, dx, dy) in [
        ("UP", 11.5, -13.0),
        ("LEFT", 0.0, 0.0),
        ("DOWN", 11.5, 0.0),
        ("RIGHT", 23.0, 0.0),
    ] {
        cap(ui, ax + dx, y + dy, 11.5, key);
    }
    // Every action's key (the drawing's legend, and the keys it leaves
    // out).
    y += 17.0;
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
