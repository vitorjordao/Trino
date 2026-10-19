//! The data a screen is made of: a flat, pre-order list of [`UiNode`]s
//! (each knows its parent and where its subtree ends), the screen's
//! [`Theme`] and its header ([`UiScreen`]). The same types come out of
//! `cargo xtask ui export` (static Rust data) and out of a baked `TUIL`
//! blob ([`UiScreen::parse`]), so a game reads both the same way.

use crate::caps::Family;
use crate::math::Color;
use crate::render::{FontId, ModelId, Screen, SpriteId, TextAlign};

/// "No node": a missing parent, focus target or override.
pub const NONE: u16 = u16::MAX;

/// The targets a node shows on (`visible_on`), as a bit set.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct Targets(pub u8);

impl Targets {
    pub const PC: Targets = Targets(1);
    pub const N64: Targets = Targets(2);
    pub const N3DS: Targets = Targets(4);
    pub const WEB: Targets = Targets(8);
    pub const ALL: Targets = Targets(15);
    pub const NONE: Targets = Targets(0);

    /// The bit of one target family.
    pub const fn of(family: Family) -> Targets {
        match family {
            Family::Pc => Targets::PC,
            Family::N64 => Targets::N64,
            Family::N3ds => Targets::N3DS,
            Family::Web => Targets::WEB,
        }
    }

    pub const fn contains(self, family: Family) -> bool {
        self.0 & Targets::of(family).0 != 0
    }

    pub const fn union(self, other: Targets) -> Targets {
        Targets(self.0 | other.0)
    }
}

impl Default for Targets {
    fn default() -> Self {
        Targets::ALL
    }
}

/// A stack's or a list's direction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Dir {
    Horizontal,
    #[default]
    Vertical,
}

/// Where children sit across a stack (`Stretch`: as wide as the stack).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Align {
    #[default]
    Start,
    Center,
    End,
    Stretch,
}

/// How a stack spends spare room along its direction.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Justify {
    #[default]
    Start,
    Center,
    End,
    SpaceBetween,
}

/// Where a node sits in its parent's box: nine points, or stretched across
/// one axis (or both) of the parent's content box. `offset` moves the node
/// in screen axes (x right, y down) from there; on a stretched axis it
/// insets both sides instead.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Anchor {
    #[default]
    TopLeft,
    Top,
    TopRight,
    Left,
    Center,
    Right,
    BottomLeft,
    Bottom,
    BottomRight,
    /// Full width, at the top.
    StretchTop,
    /// Full width, centered vertically.
    StretchMiddle,
    /// Full width, at the bottom.
    StretchBottom,
    /// Full height, at the left.
    StretchLeft,
    /// Full height, centered horizontally.
    StretchCenter,
    /// Full height, at the right.
    StretchRight,
    /// The parent's whole content box.
    Fill,
}

/// One axis of an [`Anchor`]: where on that axis, or stretched.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AxisAnchor {
    /// 0 = start, 0.5 = center, 1 = end.
    At(f32),
    Stretch,
}

impl Anchor {
    pub const ALL: [Anchor; 16] = [
        Anchor::TopLeft,
        Anchor::Top,
        Anchor::TopRight,
        Anchor::Left,
        Anchor::Center,
        Anchor::Right,
        Anchor::BottomLeft,
        Anchor::Bottom,
        Anchor::BottomRight,
        Anchor::StretchTop,
        Anchor::StretchMiddle,
        Anchor::StretchBottom,
        Anchor::StretchLeft,
        Anchor::StretchCenter,
        Anchor::StretchRight,
        Anchor::Fill,
    ];

    /// The horizontal and vertical parts.
    pub const fn axes(self) -> (AxisAnchor, AxisAnchor) {
        use AxisAnchor::{At, Stretch};
        match self {
            Anchor::TopLeft => (At(0.0), At(0.0)),
            Anchor::Top => (At(0.5), At(0.0)),
            Anchor::TopRight => (At(1.0), At(0.0)),
            Anchor::Left => (At(0.0), At(0.5)),
            Anchor::Center => (At(0.5), At(0.5)),
            Anchor::Right => (At(1.0), At(0.5)),
            Anchor::BottomLeft => (At(0.0), At(1.0)),
            Anchor::Bottom => (At(0.5), At(1.0)),
            Anchor::BottomRight => (At(1.0), At(1.0)),
            Anchor::StretchTop => (Stretch, At(0.0)),
            Anchor::StretchMiddle => (Stretch, At(0.5)),
            Anchor::StretchBottom => (Stretch, At(1.0)),
            Anchor::StretchLeft => (At(0.0), Stretch),
            Anchor::StretchCenter => (At(0.5), Stretch),
            Anchor::StretchRight => (At(1.0), Stretch),
            Anchor::Fill => (Stretch, Stretch),
        }
    }
}

/// A node's size on one axis, in virtual units (see [`UiScreen::reference`]).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Size {
    /// As big as its content (text, sprite, children) plus padding.
    #[default]
    Fit,
    /// A fixed size in virtual units.
    Px(u16),
    /// A share of the parent's content box, in percent.
    Percent(u16),
    /// The room left: shared by every `Fill` child along a stack, the whole
    /// content box elsewhere.
    Fill,
}

/// What a text does when it is wider (or, wrapped, taller) than its box.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Overflow {
    /// Draw past the box; `ui check` reports it.
    #[default]
    Visible,
    /// Cut at the last whole character that fits; reported too.
    Clip,
    /// Cut and end with `...`: chosen on purpose, not an error.
    Ellipsis,
    /// Break lines at spaces; reported when the lines outgrow the box.
    Wrap,
}

/// A text's vertical place in its box.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum VAlign {
    Top,
    #[default]
    Middle,
    Bottom,
}

/// How an image fills its box (a nine-slice image always stretches).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum ImageFit {
    /// Scaled to the box on both axes.
    #[default]
    Stretch,
    /// As big as fits with its aspect, centered.
    Contain,
    /// The biggest whole-number scale that fits, centered: pixel art.
    Integer,
}

/// How virtual units become pixels.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Scaling {
    /// A whole number of pixels per unit (the largest that fits the
    /// reference size in the screen, at least 1): pixel art and bitmap
    /// fonts stay crisp. The consoles' 320×240 and 400×240 screens are 1.
    #[default]
    Integer,
    /// Exactly the fit (text may land between pixels).
    Fractional,
}

/// What a node is. Containers place their children; leaves draw. Any node
/// may have children: a leaf lays them out like an [`Overlay`](Self::Overlay)
/// inside its content box (a panel is a `Rect` with a `Stack` in it).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum NodeKind {
    /// Children placed by their anchors.
    Overlay,
    /// Children one after the other.
    Stack {
        dir: Dir,
        gap: i16,
        align: Align,
        justify: Justify,
    },
    /// Children in rows of `cols` equal columns.
    Grid { cols: u8, gap: (i16, i16) },
    /// A stack that scrolls along `dir`; children outside its box are not
    /// drawn (2D has no scissor on the consoles), the focus scrolls them in.
    Scroll { dir: Dir, gap: i16 },
    /// The first child repeated as many times as the int bound to `bind`
    /// says (at most `max`), stacked along `dir`.
    List { dir: Dir, gap: i16, max: u16 },
    /// A line (or wrapped lines) of text.
    Text,
    /// A sprite; `src` is its size in pixels (filled in by the bake), `nine`
    /// the nine-slice insets (left, top, right, bottom; all 0 = plain).
    Image {
        sprite: SpriteId,
        src: (u16, u16),
        nine: [u8; 4],
        fit: ImageFit,
    },
    /// A solid rectangle (`color`), with an inner border.
    Rect { border: u8, border_color: Color },
    /// Focusable; A, a click or a tap sends its `action`.
    Button,
    /// Focusable; flips the bool bound to `bind`.
    Toggle,
    /// Focusable; left/right (or a drag) moves the int bound to `bind`.
    Slider { min: i32, max: i32, step: i32 },
    /// A 3D model in a box, drawn through the normal 3D pipeline in front of
    /// everything: `extent` model units (×100) fill the box's height,
    /// `lift` (×100) is the model point put at the box's center, `yaw`
    /// and `pitch` in degrees, `spin` degrees per second.
    Model3d {
        model: ModelId,
        yaw: i16,
        pitch: i16,
        spin: i16,
        extent: u16,
        lift: i16,
    },
}

impl NodeKind {
    /// The node's name in `*.ui.ron` files and tools.
    pub const fn name(&self) -> &'static str {
        match self {
            NodeKind::Overlay => "Overlay",
            NodeKind::Stack { .. } => "Stack",
            NodeKind::Grid { .. } => "Grid",
            NodeKind::Scroll { .. } => "Scroll",
            NodeKind::List { .. } => "List",
            NodeKind::Text => "Text",
            NodeKind::Image { .. } => "Image",
            NodeKind::Rect { .. } => "Rect",
            NodeKind::Button => "Button",
            NodeKind::Toggle => "Toggle",
            NodeKind::Slider { .. } => "Slider",
            NodeKind::Model3d { .. } => "Model3d",
        }
    }

    /// Takes the focus (pad, mouse, touch).
    pub const fn is_focusable(&self) -> bool {
        matches!(
            self,
            NodeKind::Button | NodeKind::Toggle | NodeKind::Slider { .. }
        )
    }

    /// Draws a label (`text`, `key`, `bind`).
    pub const fn has_text(&self) -> bool {
        matches!(
            self,
            NodeKind::Text | NodeKind::Button | NodeKind::Toggle | NodeKind::Slider { .. }
        )
    }
}

/// One node of a screen. Strings borrow the screen's data: a static slice
/// from `ui export` or a baked blob.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct UiNode<'a> {
    pub kind: NodeKind,
    /// Optional name: focus overrides, `ui.set_focus`, events, tools.
    pub id: &'a str,
    /// The parent's index ([`NONE`] for the root, node 0).
    pub parent: u16,
    /// The index after this node's subtree (children are `index+1..end`).
    pub end: u16,
    /// Which screen the node is on. On targets with one screen, `Bottom`
    /// nodes are laid out and drawn on the main screen.
    pub screen: Screen,
    pub anchor: Anchor,
    pub offset: (i16, i16),
    pub size: (Size, Size),
    /// 0 = no limit.
    pub min: (u16, u16),
    pub max: (u16, u16),
    /// Left, top, right, bottom.
    pub padding: [u8; 4],
    /// Placed against the whole screen, past the safe area (backgrounds).
    pub bleed: bool,
    pub visible_on: Targets,
    /// Fill (Rect, Button, containers), text color (Text); `None` = theme.
    pub color: Option<Color>,
    /// `None` = the theme's font.
    pub font: Option<FontId>,
    /// Text pixel scale in virtual units; 0 = the theme's.
    pub text_scale: u8,
    pub align: TextAlign,
    pub valign: VAlign,
    pub overflow: Overflow,
    /// The text in the default language (and the template for `bind`: `{}`
    /// is replaced by the bound value; without `{}` the value follows the
    /// text after a space).
    pub text: &'a str,
    /// A string-table key ([`UiStrings`](super::UiStrings)): the text in the
    /// current language when the table has it.
    pub key: &'a str,
    /// A value key: the int or text shown, the bool a toggle flips, the int
    /// a slider moves, a list's item count.
    pub bind: &'a str,
    /// Sent on activation ([`UiEvent::Click`](super::UiEvent::Click)).
    pub action: &'a str,
    /// A bool value key: the node (and its subtree) shows only while true.
    pub visible_if: &'a str,
    /// A bool value key: the node takes the focus only while true.
    pub enabled_if: &'a str,
    /// Explicit focus targets for up, down, left, right ([`NONE`] =
    /// by geometry).
    pub nav: [u16; 4],
    /// A second channel for color (issue #60): a pattern or a shape shown
    /// over the node's right end while the player's color-vision mode is
    /// on ([`UiEnv::with_access`](super::UiEnv::with_access)), so a
    /// colored marker, kit or status still reads without its hue.
    pub cvd_icon: Option<CvdIcon>,
}

/// A [`UiNode::cvd_icon`]: a sprite and its size in pixels (drawn at the
/// layout's scale).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct CvdIcon {
    pub sprite: SpriteId,
    pub size: (u16, u16),
}

impl UiNode<'static> {
    /// A `Fit`-sized `Overlay` at the top-left, on every target: the base
    /// for `UiNode { .., ..UiNode::DEFAULT }`.
    pub const DEFAULT: UiNode<'static> = UiNode {
        kind: NodeKind::Overlay,
        id: "",
        parent: NONE,
        end: 1,
        screen: Screen::Top,
        anchor: Anchor::TopLeft,
        offset: (0, 0),
        size: (Size::Fit, Size::Fit),
        min: (0, 0),
        max: (0, 0),
        padding: [0; 4],
        bleed: false,
        visible_on: Targets::ALL,
        color: None,
        font: None,
        text_scale: 0,
        align: TextAlign::Left,
        valign: VAlign::Middle,
        overflow: Overflow::Visible,
        text: "",
        key: "",
        bind: "",
        action: "",
        visible_if: "",
        enabled_if: "",
        nav: [NONE; 4],
        cvd_icon: None,
    };
}

/// A screen's colors, font and text scale (nodes override per field).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Theme {
    pub font: FontId,
    pub text_scale: u8,
    pub text: Color,
    /// Disabled text, slider tracks, unchecked boxes.
    pub dim: Color,
    /// A `Rect` without a color.
    pub panel: Color,
    pub button: Color,
    pub button_text: Color,
    /// The focused control's fill and text.
    pub focus: Color,
    pub focus_text: Color,
    /// The focus outline, checks, slider fills.
    pub accent: Color,
    /// A control held down (pointer or A).
    pub pressed: Color,
}

impl Theme {
    /// White text on dark blue panels, a gold focus, the engine's 5×7 font.
    pub const DEFAULT: Theme = Theme {
        font: FontId::from_path("fonts/trino5x7"),
        text_scale: 1,
        text: Color::WHITE,
        dim: Color::rgb(130, 140, 165),
        panel: Color::rgba(12, 16, 28, 220),
        button: Color::rgb(40, 48, 76),
        button_text: Color::WHITE,
        focus: Color::rgb(255, 214, 90),
        focus_text: Color::rgb(24, 20, 12),
        accent: Color::rgb(255, 244, 200),
        pressed: Color::rgb(255, 160, 60),
    };
}

impl Theme {
    /// The engine's high-contrast colors (WCAG AAA: 7:1 or more for text):
    /// white on black panels, a yellow focus with black text, white
    /// outlines. A project overrides them with `ui/themes/high_contrast.ron`.
    pub const HIGH_CONTRAST: Theme = Theme {
        font: FontId::from_path("fonts/trino5x7"),
        text_scale: 1,
        text: Color::WHITE,
        dim: Color::rgb(200, 200, 200),
        panel: Color::rgb(0, 0, 0),
        button: Color::rgb(0, 0, 0),
        button_text: Color::WHITE,
        focus: Color::rgb(255, 230, 0),
        focus_text: Color::rgb(0, 0, 0),
        accent: Color::rgb(255, 230, 0),
        pressed: Color::WHITE,
    };

    /// This theme with `contrast`'s colors (its font and text scale stay:
    /// the layout measures with them).
    pub const fn with_colors_of(&self, contrast: &Theme) -> Theme {
        Theme {
            font: self.font,
            text_scale: self.text_scale,
            ..*contrast
        }
    }
}

impl Default for Theme {
    fn default() -> Self {
        Theme::DEFAULT
    }
}

/// Where a screen's nodes live.
#[derive(Clone, Copy, Debug)]
pub enum Nodes<'a> {
    /// Static data (`cargo xtask ui export`).
    Slice(&'a [UiNode<'a>]),
    /// A baked `TUIL` blob, decoded node by node.
    Baked(super::tuil::Records<'a>),
}

/// One screen: its header and its nodes (node 0 is the root).
#[derive(Clone, Copy, Debug)]
pub struct UiScreen<'a> {
    pub name: &'a str,
    /// The size the screen was designed for, in virtual units: the scale
    /// is the largest that fits it in the target's screen ([`Scaling`]),
    /// and the layout fills the rest (anchors reach the real edges).
    pub reference: (u16, u16),
    pub scaling: Scaling,
    /// Focus moves wrap around rows and columns.
    pub wrap: bool,
    /// The node focused first ([`NONE`]: the first focusable one).
    pub focus: u16,
    /// Sent with [`UiEvent::Back`](super::UiEvent::Back) (B, Escape).
    pub on_back: &'a str,
    pub theme: Theme,
    pub nodes: Nodes<'a>,
}

impl<'a> UiScreen<'a> {
    /// How many nodes.
    pub fn len(&self) -> usize {
        match self.nodes {
            Nodes::Slice(s) => s.len(),
            Nodes::Baked(r) => r.len(),
        }
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Node `i` (panics past [`len`](Self::len)).
    pub fn node(&self, i: usize) -> UiNode<'a> {
        match self.nodes {
            Nodes::Slice(s) => s[i],
            Nodes::Baked(r) => r.node(i),
        }
    }

    /// The first node named `id`.
    pub fn find(&self, id: &str) -> Option<u16> {
        (0..self.len())
            .find(|&i| self.node(i).id == id)
            .map(|i| i as u16)
    }

    /// Whether the tree is well formed: node 0 is the only root, every
    /// parent comes before its children, every subtree nests in its
    /// parent's, focus targets are in range. Baked screens are checked by
    /// [`parse`](Self::parse); exported ones by the tests that hold them.
    pub fn check_tree(&self) -> Result<(), super::UiError> {
        let n = self.len();
        if n == 0 || n >= NONE as usize {
            return Err(super::UiError::BadTree);
        }
        for i in 0..n {
            let node = self.node(i);
            let end = node.end as usize;
            if end <= i || end > n {
                return Err(super::UiError::BadTree);
            }
            if i == 0 {
                if node.parent != NONE || end != n {
                    return Err(super::UiError::BadTree);
                }
            } else {
                let p = node.parent as usize;
                if p >= i || end > self.node(p).end as usize {
                    return Err(super::UiError::BadTree);
                }
            }
            if node.nav.iter().any(|&t| t != NONE && t as usize >= n) {
                return Err(super::UiError::BadTree);
            }
        }
        if self.focus != NONE && self.focus as usize >= n {
            return Err(super::UiError::BadTree);
        }
        Ok(())
    }

    /// The children of node `i`, in order.
    pub fn children(&self, i: usize) -> Children<'_, 'a> {
        Children {
            screen: self,
            next: i + 1,
            end: self.node(i).end as usize,
        }
    }
}

/// Iterator over a node's children ([`UiScreen::children`]).
pub struct Children<'s, 'a> {
    screen: &'s UiScreen<'a>,
    next: usize,
    end: usize,
}

impl Iterator for Children<'_, '_> {
    type Item = usize;

    fn next(&mut self) -> Option<usize> {
        if self.next >= self.end {
            return None;
        }
        let i = self.next;
        // A malformed end can't loop: subtrees always move forward.
        self.next = (self.screen.node(i).end as usize).max(i + 1);
        Some(i)
    }
}
