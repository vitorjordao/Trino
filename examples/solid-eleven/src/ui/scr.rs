//! The menus as engine UI screens (issue #95): each screen is a
//! `ui/<name>.ui.ron` exported to static Rust (`<name>_gen.rs`), laid out
//! by the engine per target and at the player's text scale, its boxes
//! growing with their text and its long lists scrolling.
//!
//! The game keeps what it had: its cursors (`g.cur`), its navigation
//! (`game::menu`) and its tap ids. A screen only shows them:
//!
//! - its texts come from the game's string tables (`key`) and, for what
//!   changes, from a [`UiData`] the screen answers by key and list item
//!   ([`show`]'s `values`): nothing is copied into the [`MenuUi`];
//! - the cursor is the UI's focus ([`Ui::set_focus_item`]), scrolled into
//!   view;
//! - tap boxes are the laid-out rects of the rows ([`hit`]), so no
//!   coordinate is written twice;
//! - what the UI system has no node for (crests, portraits, bars, the
//!   radar, slanted headers) is drawn into a node's rect with the
//!   immediate toolkit ([`art`]): the node sizes it, the art fills it.

use trino_core::ui::{NO_ITEM, TEXT_LEN, UiData, UiScreen, UiValue, text_value};
use trino_core::{Color, Rect, Renderer, UiEnv, Vec2};

use crate::SolidElevenGame;
use crate::ui::i18n::UiTr;
use crate::ui::screens::{Btn, HIT_BACK, btn_prompt};
use crate::ui::{Ui, pal};

/// The menus' UI state: one for every screen (the game keeps a single
/// screen up), so the game struct grows by one, not by a screen count.
pub type MenuUi = trino_core::Ui<128>;

/// A text value (at most [`TEXT_LEN`] bytes: the tests fail a longer one
/// instead of letting the screen cut it).
pub fn txt(s: &str) -> Option<UiValue> {
    debug_assert!(
        s.len() <= TEXT_LEN,
        "{s:?} is past a UI text value's {TEXT_LEN} bytes"
    );
    Some(text_value(s))
}

/// An int value.
pub fn int(v: i32) -> Option<UiValue> {
    Some(UiValue::Int(v))
}

/// A bool value (`visible_if`, `enabled_if`).
pub fn flag(b: bool) -> Option<UiValue> {
    Some(UiValue::Bool(b))
}

/// The keys every screen answers before its own:
///
/// - `btn.a`, `btn.b`, `btn.x`, `btn.y`: the footer's button names (the
///   pad's letters, or the PC keyboard's keys);
/// - `big` / `small`: the text scale is 150 % or more / less (a row then
///   stacks its label over its value / puts them side by side).
fn common(g: &SolidElevenGame, key: &str) -> Option<UiValue> {
    let pad = g.pad_layout();
    let btn = |b| txt(btn_prompt(pad, b).0);
    match key {
        "btn.a" => btn(Btn::A),
        "btn.b" => btn(Btn::B),
        "btn.x" => btn(Btn::X),
        "btn.y" => btn(Btn::Y),
        "big" => flag(g.access.clamped().text_factor() >= 1.5),
        "small" => flag(g.access.clamped().text_factor() < 1.5),
        _ => None,
    }
}

/// A screen's values: the common keys, then the screen's own. Its
/// revision is the screen's `rev` (a hash of the state its values read,
/// [`rev`]), the common keys' inputs and `SolidElevenGame::menu_frame` (a
/// scene change, a toast, every 30 frames): the screen lays out only when
/// a value may have changed, not on every cursor move (a layout is
/// milliseconds on the N64), at most once a frame.
struct Values<'g, F: Fn(&str, u16) -> Option<UiValue>> {
    g: &'g SolidElevenGame,
    rev: u32,
    own: F,
}

impl<F: Fn(&str, u16) -> Option<UiValue>> UiData for Values<'_, F> {
    fn value(&self, key: &str, item: u16) -> Option<UiValue> {
        common(self.g, key).or_else(|| (self.own)(key, item))
    }

    fn revision(&self) -> Option<u32> {
        let g = self.g;
        let common = rev(&(g.pad_layout(), g.access.clamped().text_factor() >= 1.5));
        Some(self.rev ^ common.rotate_left(7) ^ g.menu_frame.rotate_left(17))
    }
}

/// A revision from what a screen's values read: hash every field they
/// depend on (the cursors only where a value shows the cursor's choice).
pub fn rev(state: &impl core::hash::Hash) -> u32 {
    let mut h = Fnv(0x811c_9dc5);
    state.hash(&mut h);
    h.0
}

/// FNV-1a, for the revision.
struct Fnv(u32);

impl core::hash::Hasher for Fnv {
    fn write(&mut self, bytes: &[u8]) {
        for b in bytes {
            self.0 = (self.0 ^ *b as u32).wrapping_mul(0x0100_0193);
        }
    }

    fn finish(&self) -> u64 {
        self.0 as u64
    }
}

/// Lay out and draw `screen` with the cursor on `focus` (a node id and its
/// list item). `state` is [`rev`] of what `values` reads; `values` answers
/// the screen's own keys (see [`common`]);
/// `under` draws art beneath the screen once it is laid out. Then read
/// rects for art on top and taps from `g.menu_ui`.
#[allow(clippy::too_many_arguments)]
pub fn show<F>(
    g: &mut SolidElevenGame,
    r: &mut dyn Renderer,
    screen: &UiScreen<'_>,
    focus: Option<(&str, u16)>,
    state: u32,
    values: F,
    under: impl FnOnce(&SolidElevenGame, &MenuUi, &mut dyn Renderer),
) where
    F: Fn(&SolidElevenGame, &str, u16) -> Option<UiValue>,
{
    let mut ui = core::mem::take(&mut g.menu_ui);
    match focus {
        Some((id, item)) => {
            ui.set_focus_item(screen, id, item);
        }
        None => ui.clear_focus(),
    }
    {
        let g: &SolidElevenGame = g;
        let strings = UiTr(g.settings.lang);
        let data = Values {
            g,
            rev: state,
            own: |key: &str, item: u16| values(g, key, item),
        };
        let env = UiEnv::of(r)
            .with_strings(&strings)
            .with_access(&g.access)
            .with_data(&data);
        ui.prepare(r, screen, &env);
        under(g, &ui, r);
        ui.draw(r, screen, &env);
    }
    g.menu_ui = ui;
}

/// The immediate toolkit over node `id`'s rect (list item `item`, or
/// [`NO_ITEM`]): design units from the node's top-left at the layout's
/// scale, so art keeps its proportions inside the box the screen gave it.
/// `None` when the node isn't shown (a hidden variant, scrolled away).
pub fn art<'r>(
    r: &'r mut dyn Renderer,
    ui: &MenuUi,
    screen: &UiScreen<'_>,
    id: &str,
    item: u16,
) -> Option<(Ui<'r>, Vec2)> {
    let (on, rect) = ui.rect_of_item(screen, id, item)?;
    let scale = ui.frame(on).scale;
    let size = Vec2::new(rect.size.x / scale, rect.size.y / scale);
    let picture = r.screen_size(on).unwrap_or(Vec2::new(
        r.caps().screen_width as f32,
        r.caps().screen_height as f32,
    ));
    Some((
        Ui {
            r,
            scale,
            origin: rect.pos,
            screen: picture,
        },
        size,
    ))
}

/// Register node `id`'s rect (list item `item`) as tap target `hit`.
pub fn hit(g: &mut SolidElevenGame, screen: &UiScreen<'_>, id: &str, item: u16, hit: u8) {
    if let Some((on, rect)) = g.menu_ui.rect_of_item(screen, id, item) {
        g.hits.add(rect, on, hit);
    }
}

/// The header's back arrow and the footer's back hint go back.
pub fn back_hits(g: &mut SolidElevenGame, screen: &UiScreen<'_>) {
    hit(g, screen, "back", NO_ITEM, HIT_BACK);
    hit(g, screen, "hint_b", NO_ITEM, HIT_BACK);
}

/// The slanted header bar under the node `header`, the back arrow's cyan
/// tab under `back`, and each footer hint's slanted pill under
/// `hint_a`..`hint_y` (the shapes the reference art has and the UI system
/// draws square).
pub fn chrome(ui: &MenuUi, screen: &UiScreen<'_>, r: &mut dyn Renderer) {
    if let Some((mut a, size)) = art(r, ui, screen, "header", NO_ITEM) {
        a.slant(Rect::new(-4.0, 0.0, size.x + 10.0, size.y), pal::NAVY, 6.0);
    }
    if let Some((mut a, size)) = art(r, ui, screen, "back", NO_ITEM) {
        a.slant(Rect::new(-2.0, 0.0, size.x + 4.0, size.y), pal::CYAN, 6.0);
    }
    for (id, c) in [
        ("hint_a", pal::GREEN),
        ("hint_b", pal::RED),
        ("hint_x", pal::BLUE),
        ("hint_y", pal::YELLOW),
    ] {
        if let Some((mut a, size)) = art(r, ui, screen, id, NO_ITEM) {
            let pill = Rect::new(0.0, 0.0, size.x, size.y);
            a.slant(pill, Color::rgba(c.r / 3, c.g / 3, c.b / 3, 230), 3.0);
            a.slant_border(pill, c, 3.0, 1.0);
        }
    }
}
