//! Declarative screens (issue #58, ADR 0024): menus, HUDs and dialogs as
//! data, laid out per target, driven by the pad, the mouse and touch, drawn
//! with the 2D calls every backend has (`fill_rect`, `draw_sprite`,
//! `draw_text`) — no backend code.
//!
//! A screen is a tree of [`UiNode`]s written as `ui/<name>.ui.ron` (by
//! hand, by an agent, or by the editor's UI tab) and shipped two ways: a
//! baked `TUIL` blob (`ui/<name>`, [`UiScreen::parse`]) or static Rust from
//! `cargo xtask ui export`. [`Ui`] holds what changes: the layout, the
//! focus, scroll offsets, the values the screen shows.
//!
//! ```ignore
//! // update_with (where the platform is): `pause` is a
//! // `ScreenBlob::new("pause")`, parsed once, again after a live edit.
//! self.pause.get(platform);
//! if let Some(screen) = self.pause.screen() {
//!     for e in self.ui.update(screen, input, &self.prev, dt).iter() {
//!         match e {
//!             UiEvent::Click { action: "resume", .. } | UiEvent::Back { .. } => self.paused = false,
//!             _ => {}
//!         }
//!     }
//! }
//! // render, after the world:
//! if let Some(screen) = self.pause.screen() {
//!     self.ui.draw(renderer, screen, &UiEnv::of(renderer));
//! }
//! ```
//!
//! **Layout.** Sizes and offsets are virtual units against the screen's
//! [`reference`](UiScreen::reference) size; the scale is the largest that
//! fits it (whole pixels by default, [`Scaling`]), times the PC's interface
//! scale, and the layout fills the whole screen at that scale: anchors
//! reach the real edges of a 21:9 monitor or a phone in portrait. The root
//! keeps inside the target's safe area ([`Renderer::safe_area`]: the N64's
//! TV overscan, a phone's notch); `bleed` nodes ignore it.
//!
//! **Focus.** The D-pad, the stick and the second stick (the N64's C
//! buttons) move the focus by geometry, with per-node overrides; A accepts,
//! B (or Escape) goes back. The mouse focuses what it hovers and clicks it;
//! a touch on the 3DS bottom screen (or a phone) taps it. Pad-only targets
//! need nothing else: every control is reachable by geometry, and
//! [`Ui::unreachable`] (what `cargo xtask ui check` runs) proves it.
//!
//! **N64.** The layout is a few float operations per node, redone only when
//! the screen, its values, the language or the screen size change; a frame
//! costs its fills, sprites and glyphs. [`Caps::max_ui_nodes`] and
//! [`Caps::max_ui_draws`] are each target's budget: strict mode on the PC
//! fails a screen past them ([`Renderer::ui_stats`]).

mod asset;
mod draw;
mod layout;
mod nav;
pub mod node;
pub mod strings;
pub mod tuil;
pub mod values;

pub use asset::{ScreenBlob, StringsBlob};
pub use node::{
    Align, Anchor, AxisAnchor, CvdIcon, Dir, ImageFit, Justify, NONE, NodeKind, Nodes, Overflow,
    Scaling, Size, Targets, Theme, UiNode, UiScreen, VAlign,
};
pub use strings::{Fallback, NoStrings, StringTable, StringsError, UiStrings};
pub use values::{NO_ITEM, NoData, TEXT_LEN, UiData, UiValue, UiValues, text_value};

use crate::caps::{Caps, Family};
use crate::math::{Rect, Vec2};
use crate::render::{FontId, Renderer, Screen};

/// Pixels kept clear at each edge of a screen.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub struct Insets {
    pub left: u16,
    pub top: u16,
    pub right: u16,
    pub bottom: u16,
}

impl Insets {
    pub const ZERO: Insets = Insets {
        left: 0,
        top: 0,
        right: 0,
        bottom: 0,
    };

    /// The same margin left and right, top and bottom.
    pub const fn symmetric(x: u16, y: u16) -> Insets {
        Insets {
            left: x,
            top: y,
            right: x,
            bottom: y,
        }
    }
}

/// Why a baked screen was refused.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiError {
    BadMagic,
    UnsupportedVersion(u16),
    Truncated,
    /// A string outside the pool, or not UTF-8.
    BadString,
    /// Parents, subtrees or focus targets out of order or range.
    BadTree,
}

/// What a screen cost the last time it was drawn ([`Renderer::ui_stats`]).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct UiStats {
    /// Nodes laid out (list items count once each).
    pub nodes: u16,
    /// 2D draws: fills, sprites, glyphs.
    pub draws: u16,
    /// Texts past their box (`Visible`, `Clip`, `Wrap`).
    pub overflow: u16,
    /// Nodes past the [`Ui`]'s capacity, not laid out.
    pub dropped: u16,
}

/// Text sizes for the layout: the renderer's fonts at run time, the baked
/// tables in tools.
pub trait TextMeasure {
    /// The size `text` covers at pixel scale `scale` (`\n` starts a line).
    fn text_size(&self, font: FontId, text: &str, scale: f32) -> Vec2;
}

impl<'r> TextMeasure for dyn Renderer + 'r {
    fn text_size(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        self.measure_text(font, text, scale)
    }
}

/// Where a screen is laid out: the target, its screens and safe areas,
/// the interface scale and the language.
#[derive(Clone, Copy)]
pub struct UiEnv<'a> {
    pub family: Family,
    pub top: Vec2,
    /// The second screen (the 3DS touch screen), if there is one.
    pub bottom: Option<Vec2>,
    /// Safe areas of the top and the bottom screen, pixels.
    pub safe: [Insets; 2],
    /// The player's interface scale (the PC's
    /// [`DisplaySettings::interface_scale`](crate::DisplaySettings::interface_scale),
    /// 1 elsewhere).
    pub interface_scale: f32,
    pub strings: &'a dyn UiStrings,
    /// Values the game answers on demand ([`with_data`](Self::with_data)).
    pub data: &'a dyn UiData,
    /// The player's accessibility settings the screen honors
    /// ([`with_access`](Self::with_access)).
    pub access: UiAccess,
}

/// What a player's accessibility settings change in a screen (issue #60).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UiAccess {
    /// Every text's size times this (1 ..= 2). Where the screen scales by
    /// whole pixels ([`Scaling::Integer`]) a glyph's pixel size is rounded
    /// to a whole number, so bitmap fonts stay crisp (and the N64 keeps its
    /// fast text path): at ×1 (the consoles) 150 % and up draw at 200 %.
    pub text_scale: f32,
    /// Draw with [`contrast`](Self::contrast) (or [`Theme::HIGH_CONTRAST`]):
    /// node colors give way to the theme's, controls get outlines.
    pub high_contrast: bool,
    /// The project's high-contrast theme (`ui/themes/high_contrast.ron`).
    pub contrast: Option<Theme>,
    /// Nodes with a [`cvd_icon`](UiNode::cvd_icon) show it while this is
    /// not `Off`.
    pub cvd: crate::access::CvdMode,
}

impl UiAccess {
    /// No change: today's look.
    pub const NONE: UiAccess = UiAccess {
        text_scale: 1.0,
        high_contrast: false,
        contrast: None,
        cvd: crate::access::CvdMode::Off,
    };

    /// The theme a screen draws with.
    pub fn theme(&self, screen: &Theme) -> Theme {
        if self.high_contrast {
            screen.with_colors_of(self.contrast.as_ref().unwrap_or(&Theme::HIGH_CONTRAST))
        } else {
            *screen
        }
    }

    /// The factor a text's size is multiplied by on a frame whose layout
    /// scale is `frame_scale` pixels per unit.
    pub fn text_mul(&self, scaling: Scaling, frame_scale: f32) -> f32 {
        let t = self.text_scale.clamp(1.0, 2.0);
        if t == 1.0 {
            return 1.0;
        }
        if scaling == Scaling::Integer && frame_scale >= 1.0 {
            // Whole glyph pixels: `base × mul × frame_scale` is a whole
            // number for any whole `base`.
            let px = ((t * frame_scale) + 0.5) as i32 as f32;
            return (px / frame_scale).max(1.0);
        }
        t
    }
}

impl Default for UiAccess {
    fn default() -> Self {
        Self::NONE
    }
}

impl UiEnv<'static> {
    /// The renderer's target, screens and safe areas; scale 1, no string
    /// table.
    pub fn of(renderer: &dyn Renderer) -> UiEnv<'static> {
        let caps = renderer.caps();
        UiEnv {
            family: caps.family,
            top: renderer.screen_size(Screen::Top).unwrap_or(Vec2::new(
                caps.screen_width as f32,
                caps.screen_height as f32,
            )),
            bottom: renderer.screen_size(Screen::Bottom),
            safe: [
                renderer.safe_area(Screen::Top),
                renderer.safe_area(Screen::Bottom),
            ],
            interface_scale: 1.0,
            strings: &NoStrings,
            data: &NoData,
            access: UiAccess::NONE,
        }
    }

    /// A target's own screens without a renderer (tools, tests): `caps`'
    /// screen, the 3DS's 320×240 bottom screen, its safe area.
    pub fn of_caps(caps: &Caps) -> UiEnv<'static> {
        UiEnv {
            family: caps.family,
            top: Vec2::new(caps.screen_width as f32, caps.screen_height as f32),
            bottom: (caps.family == Family::N3ds).then_some(Vec2::new(320.0, 240.0)),
            safe: [
                Insets::symmetric(caps.safe_area.0 as u16, caps.safe_area.1 as u16),
                Insets::ZERO,
            ],
            interface_scale: 1.0,
            strings: &NoStrings,
            data: &NoData,
            access: UiAccess::NONE,
        }
    }
}

impl<'a> UiEnv<'a> {
    /// The same for a player's settings: the text scale, the high-contrast
    /// theme, the color-vision mode (`platform.access()`). The project's
    /// own high-contrast theme comes from its `access/data`
    /// ([`with_contrast_theme`](Self::with_contrast_theme)).
    pub fn with_access(self, settings: &crate::access::AccessSettings) -> UiEnv<'a> {
        let s = settings.clamped();
        UiEnv {
            access: UiAccess {
                text_scale: s.text_factor(),
                high_contrast: s.high_contrast,
                cvd: s.cvd,
                ..self.access
            },
            ..self
        }
    }

    /// The same with a project's high-contrast theme (used while
    /// `high_contrast` is on): `access.or_empty().contrast_theme()`.
    pub fn with_contrast_theme(self, theme: Option<Theme>) -> UiEnv<'a> {
        UiEnv {
            access: UiAccess {
                contrast: theme,
                ..self.access
            },
            ..self
        }
    }

    /// The same with a string table.
    pub fn with_strings<'b>(self, strings: &'b dyn UiStrings) -> UiEnv<'b>
    where
        'a: 'b,
    {
        UiEnv { strings, ..self }
    }

    /// The same with the game's values ([`UiData`]): lists and texts as
    /// long as the game's data, nothing copied into the [`Ui`].
    pub fn with_data<'b>(self, data: &'b dyn UiData) -> UiEnv<'b>
    where
        'a: 'b,
    {
        UiEnv { data, ..self }
    }

    /// The same at an interface scale: the PC's
    /// `platform.display().map_or(1.0, |d| d.settings.interface_scale())`.
    pub fn with_interface_scale(self, scale: f32) -> UiEnv<'a> {
        UiEnv {
            interface_scale: if scale > 0.0 { scale } else { 1.0 },
            ..self
        }
    }
}

/// Slot flags ([`Slot::flags`]).
pub mod flags {
    /// Outside its scroll box: laid out, not drawn, still reachable.
    pub const CLIPPED: u8 = 1;
    /// Its text is past its box.
    pub const OVERFLOW: u8 = 2;
    /// Its text was cut with `...` (an `Ellipsis` node).
    pub const TRUNCATED: u8 = 4;
    /// A control whose `enabled_if` is false.
    pub const DISABLED: u8 = 8;
    /// Takes the focus (a control, enabled).
    pub const FOCUSABLE: u8 = 16;
}

/// One laid-out node: a node of the screen, once per list item it is in.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Slot {
    pub node: u16,
    /// The list item ([`NO_ITEM`] outside lists).
    pub item: u16,
    /// The parent's slot ([`NONE`] for the root).
    pub parent: u16,
    /// Where it is drawn (the main screen on one-screen targets).
    pub on: Screen,
    pub flags: u8,
}

impl Slot {
    const EMPTY: Slot = Slot {
        node: NONE,
        item: NO_ITEM,
        parent: NONE,
        on: Screen::Top,
        flags: 0,
    };

    pub fn has(&self, flag: u8) -> bool {
        self.flags & flag != 0
    }
}

/// How one screen of the target maps virtual units to pixels.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Frame {
    /// Pixels per virtual unit.
    pub scale: f32,
    /// The whole screen, virtual units.
    pub size: Vec2,
    /// The safe area, virtual units.
    pub safe: Rect,
    pub present: bool,
}

impl Frame {
    const NONE: Frame = Frame {
        scale: 1.0,
        size: Vec2::ZERO,
        safe: Rect::new(0.0, 0.0, 0.0, 0.0),
        present: false,
    };
}

/// What happened in a frame of [`Ui::update`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum UiEvent<'a> {
    /// A control was activated (A, a click, a tap): its `action`.
    Click {
        action: &'a str,
        id: &'a str,
        item: u16,
    },
    /// A toggle or slider changed the value bound to `bind`.
    Changed {
        bind: &'a str,
        value: i32,
        item: u16,
    },
    /// B or Escape: the screen's `on_back`.
    Back { action: &'a str },
    /// The focus moved (a cue for a sound).
    Moved { id: &'a str, item: u16 },
}

/// Most events one frame reports.
pub const MAX_EVENTS: usize = 8;

/// A frame's events, in order.
#[derive(Clone, Copy, Debug)]
pub struct UiEvents<'a> {
    list: [Option<UiEvent<'a>>; MAX_EVENTS],
    len: usize,
}

impl<'a> UiEvents<'a> {
    pub const fn new() -> Self {
        UiEvents {
            list: [None; MAX_EVENTS],
            len: 0,
        }
    }

    fn push(&mut self, e: UiEvent<'a>) {
        if self.len < MAX_EVENTS {
            self.list[self.len] = Some(e);
            self.len += 1;
        }
    }

    pub fn iter(&self) -> impl Iterator<Item = UiEvent<'a>> + '_ {
        self.list[..self.len].iter().flatten().copied()
    }

    pub fn len(&self) -> usize {
        self.len
    }

    pub fn is_empty(&self) -> bool {
        self.len == 0
    }

    /// The first click's action.
    pub fn clicked(&self) -> Option<&'a str> {
        self.iter().find_map(|e| match e {
            UiEvent::Click { action, .. } => Some(action),
            _ => None,
        })
    }

    /// B or Escape was pressed.
    pub fn back(&self) -> bool {
        self.iter().any(|e| matches!(e, UiEvent::Back { .. }))
    }
}

impl Default for UiEvents<'_> {
    fn default() -> Self {
        Self::new()
    }
}

/// Scroll boxes a screen may hold.
pub const MAX_SCROLLS: usize = 4;
/// Sliders a screen may hold (pointer drags; more still work by pad).
pub const MAX_SLIDERS: usize = 8;

/// A scroll box's state, virtual units.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct ScrollBox {
    node: u16,
    offset: f32,
    max: f32,
    vertical: bool,
}

impl ScrollBox {
    const NONE: ScrollBox = ScrollBox {
        node: NONE,
        offset: 0.0,
        max: 0.0,
        vertical: true,
    };
}
/// Values a [`Ui`] keeps.
pub const MAX_VALUES: usize = 24;
/// Deepest node tree laid out.
pub const MAX_DEPTH: usize = 24;

/// The state of one screen on one target: up to `N` laid-out nodes (list
/// items count once each), their pixel rects, the focus, scroll offsets and
/// the screen's values. Keep one per open screen (in the game struct, not
/// on the stack: about 2 KB at N = 64).
#[derive(Clone)]
pub struct Ui<const N: usize = 64> {
    slots: [Slot; N],
    rects: [Rect; N],
    count: usize,
    frames: [Frame; 2],
    focus: Option<(u16, u16)>,
    pressed: Option<(u16, u16)>,
    pointer_last: [Option<Vec2>; 2],
    scroll: [ScrollBox; MAX_SCROLLS],
    /// Slider tracks, pixels: (node, item, rect).
    tracks: [(u16, u16, Rect); MAX_SLIDERS],
    values: UiValues<MAX_VALUES>,
    hold_dir: Option<u8>,
    hold_t: f32,
    accept_down: bool,
    time: f32,
    key: [u32; 8],
    laid_out: bool,
    name: Option<u32>,
    stats: UiStats,
    /// The last target laid out for (no strings): what `update` lays out
    /// for when no draw has yet ([`CellMeasure`]).
    last_env: Option<EnvHint>,
    /// Text sizes times this ([`UiAccess::text_mul`] at the last layout).
    text_mul: f32,
    /// The focus was set from outside: scroll it into view at the next
    /// draw.
    reveal: bool,
    /// Label sizes measured by the current layout, by node and list item
    /// (issue #95): a fit size asks a text at every level of its tree, and
    /// building a label (a value, a format) is the expensive part on the
    /// N64. Cleared at each layout.
    text_memo: [core::cell::Cell<(layout::MemoKey, Vec2)>; N],
    /// Outer sizes measured by the current layout, by node, item and the
    /// room offered: a fit size measures its content once for its width
    /// and once for its height, so nested fit containers would measure
    /// their subtrees 2^depth times.
    size_memo: [core::cell::Cell<(layout::MemoKey, Vec2)>; N],
}

/// A [`UiEnv`] without its string table, kept across frames.
#[derive(Clone, Copy, PartialEq, Debug)]
pub(crate) struct EnvHint {
    family: Family,
    top: Vec2,
    bottom: Option<Vec2>,
    safe: [Insets; 2],
    interface_scale: f32,
    access: UiAccess,
}

impl EnvHint {
    pub(crate) fn of(env: &UiEnv<'_>) -> EnvHint {
        EnvHint {
            family: env.family,
            top: env.top,
            bottom: env.bottom,
            safe: env.safe,
            interface_scale: env.interface_scale,
            access: env.access,
        }
    }

    /// A screen's reference size on the PC, before any target is known.
    pub(crate) fn reference(screen: &UiScreen<'_>) -> EnvHint {
        EnvHint {
            family: Family::Pc,
            top: Vec2::new(screen.reference.0 as f32, screen.reference.1 as f32),
            bottom: None,
            safe: [Insets::ZERO; 2],
            interface_scale: 1.0,
            access: UiAccess::NONE,
        }
    }

    pub(crate) fn env(self) -> UiEnv<'static> {
        UiEnv {
            family: self.family,
            top: self.top,
            bottom: self.bottom,
            safe: self.safe,
            interface_scale: self.interface_scale,
            strings: &NoStrings,
            data: &NoData,
            access: self.access,
        }
    }
}

/// Text measured as the engine font's 6 × 10 cell: how [`Ui::update`]
/// lays a screen out before its first draw (headless tests, a game's
/// update before its render), so the pad already moves by geometry.
pub struct CellMeasure;

impl TextMeasure for CellMeasure {
    fn text_size(&self, _font: FontId, text: &str, scale: f32) -> Vec2 {
        let mut widest = 0usize;
        let mut lines = 0usize;
        for line in text.split('\n') {
            widest = widest.max(line.chars().count());
            lines += 1;
        }
        Vec2::new(widest as f32 * 6.0 * scale, lines as f32 * 10.0 * scale)
    }
}

impl<const N: usize> Default for Ui<N> {
    fn default() -> Self {
        Self::new()
    }
}

impl<const N: usize> Ui<N> {
    pub const fn new() -> Self {
        Ui {
            slots: [Slot::EMPTY; N],
            rects: [Rect::new(0.0, 0.0, 0.0, 0.0); N],
            count: 0,
            frames: [Frame::NONE; 2],
            focus: None,
            pressed: None,
            pointer_last: [None; 2],
            scroll: [ScrollBox::NONE; MAX_SCROLLS],
            tracks: [(NONE, 0, Rect::new(0.0, 0.0, 0.0, 0.0)); MAX_SLIDERS],
            values: UiValues::new(),
            hold_dir: None,
            hold_t: 0.0,
            accept_down: false,
            time: 0.0,
            key: [0; 8],
            laid_out: false,
            name: None,
            stats: UiStats {
                nodes: 0,
                draws: 0,
                overflow: 0,
                dropped: 0,
            },
            last_env: None,
            text_mul: 1.0,
            reveal: false,
            text_memo: [const { core::cell::Cell::new((layout::MemoKey::EMPTY, Vec2::ZERO)) }; N],
            size_memo: [const { core::cell::Cell::new((layout::MemoKey::EMPTY, Vec2::ZERO)) }; N],
        }
    }

    // ---- values -------------------------------------------------------

    /// Set an int value (`Text(bind: "money")` shows it).
    pub fn set(&mut self, key: &str, value: i32) -> bool {
        self.values.put(key, NO_ITEM, UiValue::Int(value))
    }

    pub fn set_bool(&mut self, key: &str, value: bool) -> bool {
        self.values.put(key, NO_ITEM, UiValue::Bool(value))
    }

    pub fn set_text(&mut self, key: &str, value: &str) -> bool {
        self.values.put(key, NO_ITEM, text_value(value))
    }

    /// Set a value of list item `item` (a `List` node's copies read their
    /// `bind`s with their item first).
    pub fn set_item(&mut self, key: &str, item: u16, value: UiValue) -> bool {
        self.values.put(key, item, value)
    }

    pub fn get(&self, key: &str) -> Option<i32> {
        self.values.get(key, NO_ITEM).and_then(|v| v.int())
    }

    pub fn get_bool(&self, key: &str) -> bool {
        self.values.get(key, NO_ITEM).is_some_and(|v| v.truthy())
    }

    pub fn values(&self) -> &UiValues<MAX_VALUES> {
        &self.values
    }

    pub fn values_mut(&mut self) -> &mut UiValues<MAX_VALUES> {
        &mut self.values
    }

    // ---- what the layout made ------------------------------------------

    /// The laid-out slots, in drawing order.
    pub fn slots(&self) -> &[Slot] {
        &self.slots[..self.count]
    }

    /// Slot `i`'s rect, pixels of its screen.
    pub fn rect(&self, i: usize) -> Rect {
        self.rects[i]
    }

    /// How one screen maps virtual units to pixels (after a layout).
    pub fn frame(&self, screen: Screen) -> Frame {
        self.frames[screen.index() as usize]
    }

    /// The last draw's (or layout's) counters.
    pub fn stats(&self) -> UiStats {
        self.stats
    }

    /// The first slot of the node named `id` (item 0 inside a list).
    pub fn find<'a>(&self, screen: &UiScreen<'a>, id: &str) -> Option<usize> {
        let node = screen.find(id)?;
        self.slots().iter().position(|s| s.node == node)
    }

    /// Where the node named `id` is: its screen and pixel rect.
    pub fn rect_of(&self, screen: &UiScreen<'_>, id: &str) -> Option<(Screen, Rect)> {
        let i = self.find(screen, id)?;
        Some((self.slots[i].on, self.rects[i]))
    }

    /// The focused slot.
    pub fn focus_slot(&self) -> Option<usize> {
        let (node, item) = self.focus?;
        self.slots()
            .iter()
            .position(|s| s.node == node && s.item == item)
    }

    /// The focused node's id and list item.
    pub fn focused<'a>(&self, screen: &UiScreen<'a>) -> Option<(&'a str, u16)> {
        let (node, item) = self.focus?;
        ((node as usize) < screen.len()).then(|| (screen.node(node as usize).id, item))
    }

    /// Focus the node named `id` (item 0 inside a list); `false` if it
    /// isn't a laid-out, enabled control.
    pub fn set_focus(&mut self, screen: &UiScreen<'_>, id: &str) -> bool {
        match self.find(screen, id) {
            Some(i) if self.slots[i].has(flags::FOCUSABLE) => {
                self.focus = Some((self.slots[i].node, self.slots[i].item));
                self.reveal = true;
                true
            }
            _ => false,
        }
    }

    /// Focus item `item` of the node named `id` (a row of a `List`; 0 or
    /// [`NO_ITEM`] outside one) and scroll it into view at the next draw:
    /// a game that keeps its own cursor shows it through the screen (issue
    /// #95). Works before the node is laid out (the next layout places the
    /// focus); `false` when the screen has no such node.
    pub fn set_focus_item(&mut self, screen: &UiScreen<'_>, id: &str, item: u16) -> bool {
        let Some(node) = screen.find(id) else {
            return false;
        };
        // The screen's first layout must keep this focus (a new screen
        // starts over at its layout, see `layout`).
        let name = crate::asset::asset_id(screen.name);
        if self.name != Some(name) {
            self.reset();
            self.name = Some(name);
        }
        let item = self.item_of(screen, node as usize, item);
        if self.focus != Some((node, item)) {
            self.focus = Some((node, item));
            self.reveal = true;
        }
        true
    }

    /// The slot item of node `i`'s copy `item`: itself inside a `List`'s
    /// repeated child, [`NO_ITEM`] elsewhere (one copy).
    fn item_of(&self, screen: &UiScreen<'_>, i: usize, item: u16) -> u16 {
        let mut at = screen.node(i).parent;
        while at != NONE {
            if matches!(screen.node(at as usize).kind, NodeKind::List { .. }) {
                return item;
            }
            at = screen.node(at as usize).parent;
        }
        NO_ITEM
    }

    /// Where item `item` of the node named `id` is ([`NO_ITEM`] outside a
    /// list): its screen and pixel rect, `None` when it isn't laid out or
    /// a scroll box hides it (hit boxes and a game's own drawing go only
    /// where the player sees the node).
    pub fn rect_of_item(
        &self,
        screen: &UiScreen<'_>,
        id: &str,
        item: u16,
    ) -> Option<(Screen, Rect)> {
        let node = screen.find(id)?;
        let item = self.item_of(screen, node as usize, item);
        let i = self
            .slots()
            .iter()
            .position(|s| s.node == node && s.item == item)?;
        (!self.hidden(i)).then_some((self.slots[i].on, self.rects[i]))
    }

    /// No focus: nothing highlighted (a game whose cursor is on something
    /// the screen doesn't show).
    pub fn clear_focus(&mut self) {
        self.focus = None;
    }

    /// Forget the focus, presses and scroll (a screen opened again starts
    /// at its first control); values stay.
    pub fn reset(&mut self) {
        self.focus = None;
        self.pressed = None;
        self.scroll = [ScrollBox::NONE; MAX_SCROLLS];
        self.hold_dir = None;
        self.hold_t = 0.0;
        self.laid_out = false;
    }

    /// A scroll box's offset and its maximum, virtual units.
    pub fn scroll_of(&self, node: u16) -> Option<(f32, f32)> {
        self.scroll
            .iter()
            .find(|s| s.node == node && node != NONE)
            .map(|s| (s.offset, s.max))
    }

    /// Not drawn: clipped by a scroll box (or inside one that is).
    pub(super) fn hidden(&self, i: usize) -> bool {
        let mut at = i;
        loop {
            let s = self.slots[at];
            if s.has(flags::CLIPPED) {
                return true;
            }
            if s.parent == NONE {
                return false;
            }
            at = s.parent as usize;
        }
    }

    /// A slider's track (pixels), from the last layout.
    pub(super) fn track(&self, node: u16, item: u16) -> Option<Rect> {
        self.tracks
            .iter()
            .find(|t| t.0 == node && t.1 == item)
            .map(|t| t.2)
    }

    /// The first enabled control pad navigation can't reach from the first
    /// focus, if any (the layout must be done).
    pub fn unreachable(&self, screen: &UiScreen<'_>) -> Option<usize> {
        let start = self.initial_focus(screen)?;
        let mut seen = [false; N];
        let mut queue = [0u16; N];
        let (mut head, mut tail) = (0, 1);
        queue[0] = start as u16;
        seen[start] = true;
        while head < tail {
            let at = queue[head] as usize;
            head += 1;
            for dir in 0..4 {
                if let Some(next) = self.nav_target(screen, at, dir, screen.wrap)
                    && !seen[next]
                {
                    seen[next] = true;
                    queue[tail] = next as u16;
                    tail += 1;
                }
            }
        }
        (0..self.count).find(|&i| self.slots[i].has(flags::FOCUSABLE) && !seen[i])
    }
}
