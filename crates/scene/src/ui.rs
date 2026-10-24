//! UI screens as RON (issue #58, ADR 0024): `ui/<name>.ui.ron`, the
//! authored form of [`trino_core::ui::UiScreen`].
//!
//! ```ron
//! (
//!     version: 1,
//!     name: "pause",
//!     on_back: "resume",
//!     root: (
//!         children: [
//!             (kind: Rect(), anchor: Fill, bleed: true, color: Some((0, 0, 0, 160))),
//!             (
//!                 kind: Stack(gap: 4),
//!                 anchor: Center,
//!                 children: [
//!                     (kind: Text, key: "pause.title", text: "PAUSED"),
//!                     (kind: Button, id: "resume", text: "Resume", on_click: "resume"),
//!                     (kind: Button, id: "quit", text: "Quit", on_click: "quit"),
//!                 ],
//!             ),
//!         ],
//!     ),
//! )
//! ```
//!
//! Every field but `version` and `root` has a default, and saving skips
//! defaults, so files stay short and diff well. [`Flat`] turns a
//! [`ScreenDef`] into the runtime's flat node list; [`to_tuil`] bakes it
//! into the `TUIL` blob the asset pipeline ships as `ui/<name>`;
//! [`to_rust`] writes it as `no_std` Rust (`cargo xtask ui export`);
//! [`audit`] lays it out on a target the way the runtime will and reports
//! overflowing text, unreachable controls, missing glyphs and the target's
//! node and draw budgets (`cargo xtask ui check`, the editor's UI tab).
//!
//! String tables (`assets/i18n/<lang>.ron`, [`StringsDef`]) live in
//! [`crate::i18n`] (issue #59), baked into `TSTR` blobs (`i18n/<lang>`).

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fmt::Write as _;
use std::path::Path;

pub use crate::i18n::{HostStrings, Langs, StringsDef, strings_blob_path};
use serde::{Deserialize, Serialize};
use trino_core::render3d::Camera3;
use trino_core::text::BitmapFont;
use trino_core::ui::{self as core_ui, NONE, NodeKind, Nodes, UiNode, UiScreen, UiStrings, tuil};
use trino_core::{
    Caps, Color, Family, FontId, Material, ModelId, ModelParams, Renderer, Screen, SpriteId,
    SpriteParams, TextAlign as CoreTextAlign, TextParams, Transform3, Ui, UiEnv, UiStats, Vec2,
};

/// The `.ui.ron` format version. Loaders refuse newer files.
pub const UI_VERSION: u32 = 1;

/// The engine's built-in font, what a screen without a theme font uses.
pub const DEFAULT_FONT: &str = "fonts/trino5x7";

macro_rules! mirror {
    ($(#[$m:meta])* $name:ident => $core:ty { $($v:ident),+ $(,)? } default $d:ident) => {
        $(#[$m])*
        #[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash)]
        pub enum $name { $($v),+ }

        impl Default for $name {
            fn default() -> Self {
                $name::$d
            }
        }

        impl $name {
            pub const ALL: &'static [$name] = &[$($name::$v),+];

            pub fn core(self) -> $core {
                match self { $($name::$v => <$core>::$v),+ }
            }

            pub fn label(self) -> &'static str {
                match self { $($name::$v => stringify!($v)),+ }
            }

            #[allow(dead_code)]
            fn is_default(&self) -> bool {
                *self == $name::$d
            }
        }
    };
}

mirror!(
    /// A stack's direction.
    Dir => core_ui::Dir { Horizontal, Vertical } default Vertical
);
mirror!(
    /// Cross-axis placement of a stack's children.
    Align => core_ui::Align { Start, Center, End, Stretch } default Start
);
mirror!(
    /// Main-axis distribution of a stack's children.
    Justify => core_ui::Justify { Start, Center, End, SpaceBetween } default Start
);
mirror!(
    /// Where a node sits in its parent (nine points, stretched edges, fill).
    Anchor => core_ui::Anchor {
        TopLeft, Top, TopRight, Left, Center, Right, BottomLeft, Bottom, BottomRight,
        StretchTop, StretchMiddle, StretchBottom, StretchLeft, StretchCenter, StretchRight, Fill,
    } default TopLeft
);
mirror!(
    /// What text does past its box.
    Overflow => core_ui::Overflow { Visible, Clip, Ellipsis, Wrap } default Visible
);
mirror!(
    /// Vertical text placement in its box.
    VAlign => core_ui::VAlign { Top, Middle, Bottom } default Middle
);
mirror!(
    /// How an image fills its box.
    ImageFit => core_ui::ImageFit { Stretch, Contain, Integer } default Stretch
);
mirror!(
    /// Whole-pixel scaling (crisp pixel art) or any scale.
    Scaling => core_ui::Scaling { Integer, Fractional, Snap } default Integer
);
/// Horizontal text alignment.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Hash, Default)]
pub enum TextAlign {
    /// The kind's own: centered on buttons, at the start elsewhere.
    #[default]
    Auto,
    Left,
    Center,
    Right,
}

impl TextAlign {
    pub const ALL: &'static [TextAlign] = &[
        TextAlign::Auto,
        TextAlign::Left,
        TextAlign::Center,
        TextAlign::Right,
    ];

    pub fn core_for(self, kind: &Kind) -> CoreTextAlign {
        match self {
            TextAlign::Auto if *kind == Kind::Button => CoreTextAlign::Center,
            TextAlign::Auto | TextAlign::Left => CoreTextAlign::Left,
            TextAlign::Center => CoreTextAlign::Center,
            TextAlign::Right => CoreTextAlign::Right,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            TextAlign::Auto => "Auto",
            TextAlign::Left => "Left",
            TextAlign::Center => "Center",
            TextAlign::Right => "Right",
        }
    }

    fn is_default(&self) -> bool {
        *self == TextAlign::Auto
    }
}
mirror!(
    /// The screen a node is on (the 3DS's touch screen is `Bottom`).
    Side => Screen { Top, Bottom } default Top
);
mirror!(
    /// A target family, for `visible_on`.
    Platform => Family { Pc, N64, N3ds, Web } default Pc
);

/// A node's width or height.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Size {
    /// The content's size (text, children, image) plus padding.
    #[default]
    Fit,
    /// Virtual units.
    Px(u16),
    /// Of the parent's content box.
    Percent(u16),
    /// The parent's whole content box; in a stack, a share of what is left.
    Fill,
}

impl Size {
    pub fn core(self) -> core_ui::Size {
        match self {
            Size::Fit => core_ui::Size::Fit,
            Size::Px(v) => core_ui::Size::Px(v),
            Size::Percent(v) => core_ui::Size::Percent(v),
            Size::Fill => core_ui::Size::Fill,
        }
    }
}

/// A node's kind and its kind-specific settings.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(deny_unknown_fields)]
pub enum Kind {
    /// Children placed by their own anchors.
    #[default]
    Overlay,
    /// Children one after another.
    Stack {
        #[serde(default, skip_serializing_if = "Dir::is_default")]
        dir: Dir,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        gap: i16,
        #[serde(default, skip_serializing_if = "Align::is_default")]
        align: Align,
        #[serde(default, skip_serializing_if = "Justify::is_default")]
        justify: Justify,
        /// A child that doesn't fit the rest of the line starts a new one
        /// (issue #95: hint rows and chips at large text).
        #[serde(default, skip_serializing_if = "is_false")]
        wrap: bool,
    },
    /// Children in rows of `cols` equal columns.
    Grid {
        #[serde(default = "two")]
        cols: u8,
        #[serde(default, skip_serializing_if = "is_zero_pair")]
        gap: (i16, i16),
    },
    /// A stack inside a box that scrolls to follow the focus (and the
    /// wheel); what lies outside the box is hidden.
    Scroll {
        #[serde(default, skip_serializing_if = "Dir::is_default")]
        dir: Dir,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        gap: i16,
    },
    /// Its one child repeated `bind` times (at most `max`); `bind` keys
    /// inside read the item's own value (`ui.set_item`).
    List {
        #[serde(default, skip_serializing_if = "Dir::is_default")]
        dir: Dir,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        gap: i16,
        #[serde(default = "eight")]
        max: u16,
    },
    Text,
    /// A sprite, whole or nine-sliced (`nine`: left, top, right, bottom
    /// borders in source pixels). `src` is the sprite's size; the bake
    /// reads it from the image when unset.
    Image {
        sprite: String,
        #[serde(default, skip_serializing_if = "is_zero4")]
        nine: [u8; 4],
        #[serde(default, skip_serializing_if = "ImageFit::is_default")]
        fit: ImageFit,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        src: Option<(u16, u16)>,
    },
    /// A filled box (the theme's panel color without `color`), with an
    /// optional border; it can hold children.
    Rect {
        #[serde(default, skip_serializing_if = "is_zero_u8")]
        border: u8,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        border_color: Option<[u8; 4]>,
    },
    Button,
    /// A labelled check box flipping the bool `bind`.
    Toggle,
    /// A labelled track moving the int `bind` from `min` to `max`.
    Slider {
        #[serde(default)]
        min: i32,
        #[serde(default = "ten")]
        max: i32,
        #[serde(default = "one")]
        step: i32,
    },
    /// A model drawn inside the box, in front of the scene (turntables,
    /// item previews). `extent` is the model's height in hundredths of a
    /// unit that fills the box; `spin` turns it in degrees per second.
    Model3d {
        model: String,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        yaw: i16,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        pitch: i16,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        spin: i16,
        #[serde(default = "hundred")]
        extent: u16,
        #[serde(default, skip_serializing_if = "is_zero_i16")]
        lift: i16,
    },
}

impl Kind {
    /// One of each kind with default settings (the editor's "add" menu).
    pub fn all() -> Vec<Kind> {
        vec![
            Kind::Overlay,
            Kind::Stack {
                dir: Dir::Vertical,
                gap: 0,
                align: Align::Start,
                justify: Justify::Start,
                wrap: false,
            },
            Kind::Grid {
                cols: 2,
                gap: (0, 0),
            },
            Kind::Scroll {
                dir: Dir::Vertical,
                gap: 0,
            },
            Kind::List {
                dir: Dir::Vertical,
                gap: 0,
                max: 8,
            },
            Kind::Text,
            Kind::Image {
                sprite: String::new(),
                nine: [0; 4],
                fit: ImageFit::Stretch,
                src: None,
            },
            Kind::Rect {
                border: 0,
                border_color: None,
            },
            Kind::Button,
            Kind::Toggle,
            Kind::Slider {
                min: 0,
                max: 10,
                step: 1,
            },
            Kind::Model3d {
                model: String::new(),
                yaw: 0,
                pitch: 0,
                spin: 0,
                extent: 100,
                lift: 0,
            },
        ]
    }

    pub fn name(&self) -> &'static str {
        match self {
            Kind::Overlay => "Overlay",
            Kind::Stack { .. } => "Stack",
            Kind::Grid { .. } => "Grid",
            Kind::Scroll { .. } => "Scroll",
            Kind::List { .. } => "List",
            Kind::Text => "Text",
            Kind::Image { .. } => "Image",
            Kind::Rect { .. } => "Rect",
            Kind::Button => "Button",
            Kind::Toggle => "Toggle",
            Kind::Slider { .. } => "Slider",
            Kind::Model3d { .. } => "Model3d",
        }
    }

    /// Kinds that hold children.
    pub fn is_container(&self) -> bool {
        matches!(
            self,
            Kind::Overlay
                | Kind::Stack { .. }
                | Kind::Grid { .. }
                | Kind::Scroll { .. }
                | Kind::List { .. }
                | Kind::Rect { .. }
        )
    }

    pub fn is_focusable(&self) -> bool {
        matches!(self, Kind::Button | Kind::Toggle | Kind::Slider { .. })
    }

    pub fn has_text(&self) -> bool {
        matches!(
            self,
            Kind::Text | Kind::Button | Kind::Toggle | Kind::Slider { .. }
        )
    }
}

/// Explicit focus moves, by target id (empty = by geometry).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct Nav {
    #[serde(skip_serializing_if = "String::is_empty")]
    pub up: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub down: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub left: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub right: String,
}

impl Nav {
    fn is_default(&self) -> bool {
        *self == Nav::default()
    }

    pub fn targets(&self) -> [&str; 4] {
        [&self.up, &self.down, &self.left, &self.right]
    }
}

/// One node of a screen.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct NodeDef {
    pub kind: Kind,
    /// A name for focus overrides, events and the game's lookups.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub id: String,
    /// `Bottom`: the 3DS's touch screen (the main screen elsewhere). The
    /// children follow.
    #[serde(skip_serializing_if = "Side::is_default")]
    pub screen: Side,
    #[serde(skip_serializing_if = "Anchor::is_default")]
    pub anchor: Anchor,
    #[serde(skip_serializing_if = "is_zero_pair")]
    pub offset: (i16, i16),
    #[serde(skip_serializing_if = "is_fit")]
    pub size: (Size, Size),
    /// 0 = no limit.
    #[serde(skip_serializing_if = "is_zero_u16_pair")]
    pub min: (u16, u16),
    #[serde(skip_serializing_if = "is_zero_u16_pair")]
    pub max: (u16, u16),
    /// Left, top, right, bottom.
    #[serde(skip_serializing_if = "is_zero4")]
    pub padding: [u8; 4],
    /// Placed against the whole screen, past the safe area.
    #[serde(skip_serializing_if = "is_false")]
    pub bleed: bool,
    /// Empty = every target.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub visible_on: Vec<Platform>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub color: Option<[u8; 4]>,
    /// A font's logical path; empty = the theme's.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub font: String,
    /// Text pixel scale; 0 = the theme's.
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub text_scale: u8,
    #[serde(skip_serializing_if = "TextAlign::is_default")]
    pub align: TextAlign,
    #[serde(skip_serializing_if = "VAlign::is_default")]
    pub valign: VAlign,
    #[serde(skip_serializing_if = "Overflow::is_default")]
    pub overflow: Overflow,
    /// The text in the default language (`{}` = the bound value).
    #[serde(skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// A string-table key: the text in the current language.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub key: String,
    /// A value the game sets: shown, flipped, moved, or a list's count.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub bind: String,
    /// The action sent when the node is activated.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub on_click: String,
    /// A bool value: shown only while it is true.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub visible_if: String,
    /// A bool value: focusable only while it is true.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub enabled_if: String,
    #[serde(skip_serializing_if = "Nav::is_default")]
    pub nav: Nav,
    /// A sprite (logical path) shown at the node's right end while the
    /// player's color-vision mode is on: a pattern or a shape, the second
    /// channel for a color (issue #60). Its size comes from the manifest.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub cvd_icon: String,
    /// Stereo depth on the 3DS top screen (issue #63): pixels of parallax
    /// at the full 3D slider, positive out of the screen (a HUD that floats
    /// over the scene), negative into it. 0 floats with the parent.
    #[serde(skip_serializing_if = "is_zero_i8")]
    pub depth: i8,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub children: Vec<NodeDef>,
}

fn is_zero_i8(v: &i8) -> bool {
    *v == 0
}

impl NodeDef {
    pub fn new(kind: Kind) -> NodeDef {
        NodeDef {
            kind,
            ..NodeDef::default()
        }
    }

    /// The node at `path` (child indices from this node).
    pub fn at(&self, path: &[usize]) -> Option<&NodeDef> {
        match path.split_first() {
            None => Some(self),
            Some((&i, rest)) => self.children.get(i)?.at(rest),
        }
    }

    pub fn at_mut(&mut self, path: &[usize]) -> Option<&mut NodeDef> {
        match path.split_first() {
            None => Some(self),
            Some((&i, rest)) => self.children.get_mut(i)?.at_mut(rest),
        }
    }

    /// This node and its subtree, depth first.
    pub fn walk(&self, f: &mut dyn FnMut(&NodeDef)) {
        f(self);
        for c in &self.children {
            c.walk(f);
        }
    }

    /// A short label: the kind, then the id or the text.
    pub fn label(&self) -> String {
        let name = self.kind.name();
        if !self.id.is_empty() {
            format!("{name} \"{}\"", self.id)
        } else if !self.text.is_empty() {
            format!("{name} '{}'", self.text)
        } else if !self.key.is_empty() {
            format!("{name} [{}]", self.key)
        } else {
            name.to_string()
        }
    }
}

/// Overrides of the engine's default look.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Default)]
#[serde(default, deny_unknown_fields)]
pub struct ThemeDef {
    /// A font's logical path; empty = `fonts/trino5x7`.
    #[serde(skip_serializing_if = "String::is_empty")]
    pub font: String,
    /// 0 = 1.
    #[serde(skip_serializing_if = "is_zero_u8")]
    pub text_scale: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub text: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub dim: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub panel: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub button_text: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub focus_text: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub accent: Option<[u8; 4]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pressed: Option<[u8; 4]>,
}

impl ThemeDef {
    fn is_default(&self) -> bool {
        *self == ThemeDef::default()
    }

    pub fn core(&self) -> core_ui::Theme {
        self.core_over(core_ui::Theme::DEFAULT)
    }

    /// The theme with the fields it leaves out taken from `d` (the
    /// high-contrast theme file starts from `Theme::HIGH_CONTRAST`).
    pub fn core_over(&self, d: core_ui::Theme) -> core_ui::Theme {
        let c = |v: Option<[u8; 4]>, or: Color| v.map(rgba).unwrap_or(or);
        core_ui::Theme {
            font: if self.font.is_empty() {
                d.font
            } else {
                FontId::from_path(&self.font)
            },
            text_scale: if self.text_scale == 0 {
                d.text_scale
            } else {
                self.text_scale
            },
            text: c(self.text, d.text),
            dim: c(self.dim, d.dim),
            panel: c(self.panel, d.panel),
            button: c(self.button, d.button),
            button_text: c(self.button_text, d.button_text),
            focus: c(self.focus, d.focus),
            focus_text: c(self.focus_text, d.focus_text),
            accent: c(self.accent, d.accent),
            pressed: c(self.pressed, d.pressed),
        }
    }

    /// The theme's font path.
    pub fn font_path(&self) -> &str {
        if self.font.is_empty() {
            DEFAULT_FONT
        } else {
            &self.font
        }
    }
}

/// A value a preview shows (the editor, `ui shot`, `ui check`): what the
/// game would `ui.set`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum Sample {
    Int(i32),
    Bool(bool),
    Text(String),
    /// One text per list item; the list's count follows.
    Items(Vec<String>),
}

/// A screen: `ui/<name>.ui.ron`. Unknown fields are errors: a misspelt
/// one would otherwise vanish on the next save.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct ScreenDef {
    pub version: u32,
    /// Defaults to the file's stem.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub name: String,
    /// The size the screen is designed for, in virtual units.
    #[serde(default = "reference", skip_serializing_if = "is_reference")]
    pub reference: (u16, u16),
    #[serde(default, skip_serializing_if = "Scaling::is_default")]
    pub scaling: Scaling,
    /// Focus moves wrap around rows and columns.
    #[serde(default, skip_serializing_if = "is_false")]
    pub wrap: bool,
    /// The id focused first (empty: the first control).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub focus: String,
    /// The action B / Escape sends.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub on_back: String,
    #[serde(default, skip_serializing_if = "ThemeDef::is_default")]
    pub theme: ThemeDef,
    /// Values previews show (the game sets the real ones).
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub preview: BTreeMap<String, Sample>,
    /// Fills the screen's safe area (its own size and anchor are not
    /// used; `bleed: true` fills the whole screen).
    pub root: NodeDef,
}

impl Default for ScreenDef {
    fn default() -> Self {
        ScreenDef {
            version: UI_VERSION,
            name: String::new(),
            reference: reference(),
            scaling: Scaling::Integer,
            wrap: false,
            focus: String::new(),
            on_back: String::new(),
            theme: ThemeDef::default(),
            preview: BTreeMap::new(),
            root: NodeDef {
                size: (Size::Fill, Size::Fill),
                ..NodeDef::default()
            },
        }
    }
}

impl ScreenDef {
    pub fn from_ron(text: &str) -> Result<ScreenDef, String> {
        #[derive(Deserialize)]
        struct Probe {
            version: u32,
        }
        let probe: Probe =
            ron::from_str(text).map_err(|e| format!("ui screen parse error: {e}"))?;
        if probe.version > UI_VERSION {
            return Err(format!(
                "ui screen version {} is newer than this engine's ({UI_VERSION})",
                probe.version
            ));
        }
        ron::from_str(text).map_err(|e| {
            let hint = if e.to_string().contains("Expected opening `(`") {
                " (a kind with settings needs parentheses even without any: `Stack()`, \
                 `Grid()`, `Rect()`, `Slider()`)"
            } else {
                ""
            };
            format!("ui screen parse error: {e}{hint}")
        })
    }

    pub fn to_ron(&self) -> String {
        crate::to_ron_pretty(self) + "\n"
    }

    /// Loads `path`; an empty `name` becomes the file's stem.
    pub fn load(path: &Path) -> Result<ScreenDef, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
        let mut def = ScreenDef::from_ron(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        if def.name.is_empty() {
            def.name = screen_name(path);
        }
        Ok(def)
    }

    pub fn save(&self, path: &Path) -> Result<(), String> {
        crate::write_text(path, &self.to_ron())
    }

    /// Every node, depth first, with its path.
    pub fn nodes(&self) -> Vec<(Vec<usize>, &NodeDef)> {
        fn go<'a>(n: &'a NodeDef, path: &mut Vec<usize>, out: &mut Vec<(Vec<usize>, &'a NodeDef)>) {
            out.push((path.clone(), n));
            for (i, c) in n.children.iter().enumerate() {
                path.push(i);
                go(c, path, out);
                path.pop();
            }
        }
        let mut out = Vec::new();
        go(&self.root, &mut Vec::new(), &mut out);
        out
    }
}

/// `ui/pause.ui.ron` → `pause`.
pub fn screen_name(path: &Path) -> String {
    let file = path.file_name().and_then(|f| f.to_str()).unwrap_or("");
    file.strip_suffix(".ui.ron")
        .or_else(|| file.strip_suffix(".ron"))
        .unwrap_or(file)
        .to_string()
}

/// The blob a screen bakes into.
pub fn blob_path(name: &str) -> String {
    format!("ui/{name}")
}

/// Problem severity.
#[derive(Serialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "lowercase")]
pub enum Level {
    Error,
    Warning,
}

/// One problem `ui check` (and the editor) reports.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct Issue {
    pub level: Level,
    /// The node's depth-first index (`None`: the whole screen).
    pub node: Option<usize>,
    /// The node's label (`Button "resume"`).
    pub label: String,
    /// The target it happens on (`n64`, `3ds`, `pc`, `web`), if one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub platform: Option<String>,
    /// The language it happens in, if one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub lang: Option<String>,
    pub message: String,
}

impl Issue {
    fn new(level: Level, node: Option<(usize, &NodeDef)>, message: String) -> Issue {
        Issue {
            level,
            node: node.map(|n| n.0),
            label: node.map(|n| n.1.label()).unwrap_or_else(|| "screen".into()),
            platform: None,
            lang: None,
            message,
        }
    }
}

impl std::fmt::Display for Issue {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let level = match self.level {
            Level::Error => "error",
            Level::Warning => "warning",
        };
        write!(f, "{level}: {}", self.label)?;
        if let Some(n) = self.node {
            write!(f, " (#{n})")?;
        }
        if let Some(p) = &self.platform {
            write!(f, " [{p}")?;
            if let Some(l) = &self.lang {
                write!(f, ", {l}")?;
            }
            write!(f, "]")?;
        } else if let Some(l) = &self.lang {
            write!(f, " [{l}]")?;
        }
        write!(f, ": {}", self.message)
    }
}

/// A sprite's size in pixels, for `Image` nodes without `src`.
pub type SpriteSizes<'a> = &'a dyn Fn(&str) -> Option<(u16, u16)>;

/// A screen as the runtime's flat node list (depth first: a node's
/// subtree follows it).
pub struct Flat<'a> {
    pub def: &'a ScreenDef,
    pub nodes: Vec<UiNode<'a>>,
    /// Each node's path in [`ScreenDef::root`] (child indices).
    pub paths: Vec<Vec<usize>>,
    focus: u16,
}

impl<'a> Flat<'a> {
    /// Flattens `def`. Ids the nav, the focus or a sprite size need but
    /// cannot find are left at their defaults; [`validate`] reports them.
    pub fn new(def: &'a ScreenDef, sprites: SpriteSizes<'_>) -> Flat<'a> {
        let mut flat = Flat {
            def,
            nodes: Vec::new(),
            paths: Vec::new(),
            focus: NONE,
        };
        let mut path = Vec::new();
        flat.push(&def.root, NONE, Side::Top, &mut path, sprites);
        let ids: HashMap<&str, u16> = def
            .nodes()
            .iter()
            .enumerate()
            .filter(|(_, (_, n))| !n.id.is_empty())
            .map(|(i, (_, n))| (n.id.as_str(), i as u16))
            .rev() // the first of duplicates wins
            .collect();
        for (i, (_, n)) in def.nodes().iter().enumerate() {
            for (d, target) in n.nav.targets().iter().enumerate() {
                if let Some(&t) = ids.get(target) {
                    flat.nodes[i].nav[d] = t;
                }
            }
        }
        flat.focus = ids.get(def.focus.as_str()).copied().unwrap_or(NONE);
        flat
    }

    fn push(
        &mut self,
        n: &'a NodeDef,
        parent: u16,
        side: Side,
        path: &mut Vec<usize>,
        sprites: SpriteSizes<'_>,
    ) {
        let index = self.nodes.len();
        let side = if n.screen == Side::Bottom {
            Side::Bottom
        } else {
            side
        };
        let kind = match &n.kind {
            Kind::Overlay => NodeKind::Overlay,
            Kind::Stack {
                dir,
                gap,
                align,
                justify,
                wrap,
            } => NodeKind::Stack {
                dir: dir.core(),
                gap: *gap,
                align: align.core(),
                justify: justify.core(),
                wrap: *wrap,
            },
            Kind::Grid { cols, gap } => NodeKind::Grid {
                cols: *cols,
                gap: *gap,
            },
            Kind::Scroll { dir, gap } => NodeKind::Scroll {
                dir: dir.core(),
                gap: *gap,
            },
            Kind::List { dir, gap, max } => NodeKind::List {
                dir: dir.core(),
                gap: *gap,
                max: *max,
            },
            Kind::Text => NodeKind::Text,
            Kind::Image {
                sprite,
                nine,
                fit,
                src,
            } => NodeKind::Image {
                sprite: SpriteId::from_path(sprite),
                src: src.or_else(|| sprites(sprite)).unwrap_or((0, 0)),
                nine: *nine,
                fit: fit.core(),
            },
            Kind::Rect {
                border,
                border_color,
            } => NodeKind::Rect {
                border: *border,
                border_color: border_color.map(rgba).unwrap_or(Color::TRANSPARENT),
            },
            Kind::Button => NodeKind::Button,
            Kind::Toggle => NodeKind::Toggle,
            Kind::Slider { min, max, step } => NodeKind::Slider {
                min: *min,
                max: *max,
                step: *step,
            },
            Kind::Model3d {
                model,
                yaw,
                pitch,
                spin,
                extent,
                lift,
            } => NodeKind::Model3d {
                model: ModelId::from_path(model),
                yaw: *yaw,
                pitch: *pitch,
                spin: *spin,
                extent: *extent,
                lift: *lift,
            },
        };
        let visible_on = if n.visible_on.is_empty() {
            core_ui::Targets::ALL
        } else {
            n.visible_on.iter().fold(core_ui::Targets(0), |t, p| {
                t.union(core_ui::Targets::of(p.core()))
            })
        };
        self.nodes.push(UiNode {
            kind,
            id: &n.id,
            parent,
            end: 0,
            screen: side.core(),
            anchor: n.anchor.core(),
            offset: n.offset,
            size: (n.size.0.core(), n.size.1.core()),
            min: n.min,
            max: n.max,
            padding: n.padding,
            bleed: n.bleed,
            visible_on,
            color: n.color.map(rgba),
            font: (!n.font.is_empty()).then(|| FontId::from_path(&n.font)),
            text_scale: n.text_scale,
            align: n.align.core_for(&n.kind),
            valign: n.valign.core(),
            overflow: n.overflow.core(),
            text: &n.text,
            key: &n.key,
            bind: &n.bind,
            action: &n.on_click,
            visible_if: &n.visible_if,
            enabled_if: &n.enabled_if,
            nav: [NONE; 4],
            cvd_icon: (!n.cvd_icon.is_empty()).then(|| core_ui::CvdIcon {
                sprite: SpriteId::from_path(&n.cvd_icon),
                size: sprites(&n.cvd_icon).unwrap_or((8, 8)),
            }),
            depth: n.depth,
        });
        self.paths.push(path.clone());
        for (i, c) in n.children.iter().enumerate() {
            path.push(i);
            self.push(c, index as u16, side, path, sprites);
            path.pop();
        }
        self.nodes[index].end = self.nodes.len() as u16;
    }

    /// The runtime view of the screen (valid while `self` lives).
    pub fn screen(&self) -> UiScreen<'_> {
        UiScreen {
            name: &self.def.name,
            reference: self.def.reference,
            scaling: self.def.scaling.core(),
            wrap: self.def.wrap,
            focus: self.focus,
            on_back: &self.def.on_back,
            theme: self.def.theme.core(),
            nodes: Nodes::Slice(&self.nodes),
        }
    }

    /// The flat index of the node at `path`.
    pub fn index_of(&self, path: &[usize]) -> Option<usize> {
        self.paths.iter().position(|p| p == path)
    }
}

/// The images and models a screen names that the project does not have:
/// `has` says whether a logical path (`sprites/x`, `models/x`) is in its
/// manifest. Without them the node draws nothing on every target.
pub fn missing_assets(def: &ScreenDef, has: &dyn Fn(&str) -> bool) -> Vec<Issue> {
    let mut out = Vec::new();
    for (i, (_, n)) in def.nodes().iter().enumerate() {
        let path = match &n.kind {
            Kind::Image { sprite, .. } => Some(sprite),
            Kind::Model3d { model, .. } => Some(model),
            _ => None,
        };
        for path in path.into_iter().chain(Some(&n.cvd_icon)) {
            if !path.is_empty() && !has(path) {
                out.push(Issue::new(
                    Level::Error,
                    Some((i, *n)),
                    format!("{path} is not in the project's manifest"),
                ));
            }
        }
    }
    out
}

/// Structural problems: what makes a screen wrong on every target.
pub fn validate(def: &ScreenDef) -> Vec<Issue> {
    let mut out = Vec::new();
    let nodes = def.nodes();
    let mut ids: HashMap<&str, usize> = HashMap::new();
    for (i, (_, n)) in nodes.iter().enumerate() {
        if !n.id.is_empty() && ids.insert(&n.id, i).is_some() {
            out.push(Issue::new(
                Level::Error,
                Some((i, n)),
                format!("duplicate id \"{}\"", n.id),
            ));
        }
    }
    let focusable = |id: &str| ids.get(id).map(|&i| nodes[i].1.kind.is_focusable());
    if !def.focus.is_empty() {
        match focusable(&def.focus) {
            None => out.push(Issue::new(
                Level::Error,
                None,
                format!("focus: no node has id \"{}\"", def.focus),
            )),
            Some(false) => out.push(Issue::new(
                Level::Warning,
                None,
                format!("focus: \"{}\" is not a control", def.focus),
            )),
            Some(true) => {}
        }
    }
    if nodes.len() >= NONE as usize {
        out.push(Issue::new(
            Level::Error,
            None,
            format!("{} nodes: at most {}", nodes.len(), NONE - 1),
        ));
    }
    for (i, (_, n)) in nodes.iter().enumerate() {
        let at = Some((i, *n));
        let mut err = |m: String| out.push(Issue::new(Level::Error, at, m));
        for (d, target) in n.nav.targets().iter().enumerate() {
            if target.is_empty() {
                continue;
            }
            let dir = ["up", "down", "left", "right"][d];
            match focusable(target) {
                None => err(format!("nav.{dir}: no node has id \"{target}\"")),
                Some(false) => err(format!("nav.{dir}: \"{target}\" is not a control")),
                Some(true) => {}
            }
        }
        if !n.kind.is_container() && !n.children.is_empty() {
            err(format!("a {} cannot hold children", n.kind.name()));
        }
        match &n.kind {
            Kind::List { max, .. } => {
                if n.children.len() != 1 {
                    err(format!(
                        "a List repeats exactly one child (its item), not {}",
                        n.children.len()
                    ));
                }
                if *max == 0 {
                    err("a List needs max > 0".into());
                }
            }
            Kind::Grid { cols: 0, .. } => err("a Grid needs cols > 0".into()),
            Kind::Slider { min, max, step } => {
                if max <= min {
                    err(format!("a Slider needs max > min ({min}..{max})"));
                }
                if *step <= 0 {
                    err("a Slider needs step > 0".into());
                }
            }
            Kind::Image { sprite, .. } if sprite.is_empty() => {
                err("an Image needs a sprite".into())
            }
            Kind::Model3d { model, .. } if model.is_empty() => {
                err("a Model3d needs a model".into())
            }
            _ => {}
        }
        let mut warn = |m: String| out.push(Issue::new(Level::Warning, at, m));
        // A Button filling its parent both ways is a row's backdrop (the
        // focus and the tap) under sibling texts (issue #95): no label.
        let backdrop = n.kind == Kind::Button && n.size == (Size::Fill, Size::Fill);
        if matches!(n.kind, Kind::Text | Kind::Button)
            && !backdrop
            && n.text.is_empty()
            && n.key.is_empty()
            && n.bind.is_empty()
        {
            warn(format!(
                "a {} with no text, key or bind is empty",
                n.kind.name()
            ));
        }
        if matches!(n.kind, Kind::Toggle | Kind::Slider { .. }) && n.bind.is_empty() {
            warn(format!(
                "a {} without bind keeps no state the game can read",
                n.kind.name()
            ));
        }
        if matches!(n.kind, Kind::List { .. }) && n.bind.is_empty() {
            warn("a List without bind shows no items".into());
        }
        if n.kind == Kind::Button && n.on_click.is_empty() && n.id.is_empty() {
            warn("a Button without on_click or id sends an empty action".into());
        }
    }
    out
}

/// Fonts by id, as baked `TFNT` bytes.
#[derive(Default, Clone)]
pub struct Fonts {
    fonts: HashMap<FontId, Vec<u8>>,
    names: HashMap<FontId, String>,
}

impl Fonts {
    pub fn insert(&mut self, path: &str, tfnt: Vec<u8>) {
        let id = FontId::from_path(path);
        self.fonts.insert(id, tfnt);
        self.names.insert(id, path.to_string());
    }

    /// A font known to exist without its data: it measures as the engine
    /// font's 6 × 10 cell and its glyphs go unchecked.
    pub fn assume(&mut self, path: &str) {
        self.insert(path, Vec::new());
    }

    pub fn knows(&self, id: FontId) -> bool {
        self.fonts.contains_key(&id)
    }

    pub fn get(&self, id: FontId) -> Option<BitmapFont<'_>> {
        BitmapFont::parse(self.fonts.get(&id)?).ok()
    }

    pub fn contains(&self, path: &str) -> bool {
        self.fonts.contains_key(&FontId::from_path(path))
    }

    pub fn name(&self, id: FontId) -> String {
        self.names
            .get(&id)
            .cloned()
            .unwrap_or_else(|| format!("{:#010x}", id.0))
    }

    /// What the backends' `measure_text` returns; an unknown font measures
    /// as the engine font's 6 × 10 cell.
    pub fn measure(&self, id: FontId, text: &str, scale: f32) -> Vec2 {
        match self.get(id) {
            Some(f) => f.measure(text, scale),
            None => {
                let w = text.lines().map(|l| l.chars().count()).max().unwrap_or(0);
                let lines = text.split('\n').count().max(1);
                Vec2::new(w as f32 * 6.0 * scale, lines as f32 * 10.0 * scale)
            }
        }
    }
}

/// Bakes the screen into a `TUIL` blob ([`UiScreen::parse`]).
pub fn to_tuil(flat: &Flat<'_>) -> Result<Vec<u8>, String> {
    let screen = flat.screen();
    let mut pool = String::new();
    let mut at: HashMap<String, usize> = HashMap::new();
    {
        let mut add = |s: &str| {
            if !s.is_empty() && !at.contains_key(s) {
                let offset = pool.find(s).unwrap_or_else(|| {
                    pool.push_str(s);
                    pool.len() - s.len()
                });
                at.insert(s.to_string(), offset);
            }
            (0, 0)
        };
        tuil::encode_header(&screen, screen.len(), 0, &mut add);
        for i in 0..screen.len() {
            tuil::encode_record(&screen.node(i), &mut add);
        }
    }
    if pool.len() > u16::MAX as usize {
        return Err(format!(
            "{}: {} bytes of text, at most {}",
            flat.def.name,
            pool.len(),
            u16::MAX
        ));
    }
    let mut find = |s: &str| match s {
        "" => (0, 0),
        s => (at[s] as u16, s.len() as u16),
    };
    let mut out = tuil::encode_header(&screen, screen.len(), pool.len(), &mut find).to_vec();
    for i in 0..screen.len() {
        out.extend_from_slice(&tuil::encode_record(&screen.node(i), &mut find));
    }
    out.extend_from_slice(pool.as_bytes());
    UiScreen::parse(&out)
        .map_err(|e| format!("{}: baked blob does not parse: {e:?}", flat.def.name))?;
    Ok(out)
}

/// What [`audit`] measured on one target in one language.
#[derive(Clone, Debug, Default)]
pub struct Report {
    pub issues: Vec<Issue>,
    pub stats: UiStats,
    /// Nodes whose text overflows (depth-first indices), for highlights.
    pub overflow: Vec<usize>,
    /// Nodes (not `bleed`, nor inside one, nor scrolled out) that reach past
    /// the target's safe area or off the screen: `access check` fails them
    /// at 200 % text (issue #60). `ui check` leaves them to the designer.
    pub outside_safe: Vec<usize>,
    /// The smallest line of text drawn, output pixels ([`Probe::min_text_px`]).
    pub min_text_px: Option<f32>,
}

/// A target's screen: `caps`, the interface scale, a language.
#[derive(Clone, Copy)]
pub struct Target<'a> {
    pub caps: Caps,
    pub lang: Option<&'a str>,
    pub interface_scale: f32,
    /// The player's accessibility settings to lay out with (issue #60:
    /// `access check` audits every screen at 200 % text).
    pub access: core_ui::UiAccess,
}

impl<'a> Target<'a> {
    pub fn of(caps: Caps) -> Target<'a> {
        Target {
            caps,
            lang: None,
            interface_scale: 1.0,
            access: core_ui::UiAccess::NONE,
        }
    }
}

/// The family's key (`pc`, `n64`, `3ds`, `web`).
pub fn family_key(f: Family) -> &'static str {
    f.key()
}

/// Lays `flat` out on `target` with the preview values, draws it into a
/// probe (counting what the runtime would draw) and reports overflowing
/// text, controls the pad cannot reach, glyphs the fonts lack, keys the
/// language lacks and the node and draw budgets.
pub fn audit(flat: &Flat<'_>, target: Target<'_>, fonts: &Fonts, langs: &Langs) -> Report {
    let screen = flat.screen();
    let mut probe = Probe::new(target.caps, fonts);
    let strings = target.lang.map(|l| langs.strings(l));
    let mut ui: Box<Ui<1024>> = Box::new(Ui::new());
    apply_preview(&mut ui, flat.def);
    apply_access_preview(&mut ui, flat.def, target.access);
    let env = UiEnv {
        access: target.access,
        ..UiEnv::of(&probe).with_interface_scale(target.interface_scale)
    };
    let env = match &strings {
        Some(s) => env.with_strings(s),
        None => env,
    };
    ui.draw(&mut probe, &screen, &env);
    let stats = probe.stats.unwrap_or_default();
    let family = target.caps.family;
    let tag = |mut i: Issue| {
        i.platform = Some(family.key().into());
        i.lang = target.lang.map(str::to_string);
        i
    };
    let nodes = flat.def.nodes();
    let at = |i: usize| Some((i, nodes[i].1));
    let mut report = Report {
        stats,
        min_text_px: probe.min_text_px,
        ..Report::default()
    };
    let mut seen = HashSet::new();
    for s in ui.slots() {
        let i = s.node as usize;
        if s.flags & core_ui::flags::OVERFLOW != 0 && seen.insert(i) {
            report.overflow.push(i);
            let size = ui
                .rect(ui.slots().iter().position(|x| x == s).unwrap_or(0))
                .size;
            let scale = ui.frame(s.on).scale.max(1e-3);
            report.issues.push(tag(Issue::new(
                Level::Warning,
                at(i),
                format!(
                    "text overflows its {}×{} box",
                    (size.x / scale).round(),
                    (size.y / scale).round()
                ),
            )));
        }
    }
    // Nodes past the safe area (a TV's overscan, a phone's notch, the
    // screen's edge): what a bigger text pushes out.
    {
        let slots = ui.slots();
        let in_bleed = |mut k: usize| loop {
            let n = &flat.nodes[slots[k].node as usize];
            if n.bleed {
                return true;
            }
            match slots[k].parent {
                NONE => return false,
                p => k = p as usize,
            }
        };
        let mut seen = HashSet::new();
        for (k, sl) in slots.iter().enumerate() {
            if ui.is_hidden(k) || in_bleed(k) {
                continue;
            }
            let f = ui.frame(sl.on);
            let safe = trino_core::Rect::new(
                f.safe.pos.x * f.scale,
                f.safe.pos.y * f.scale,
                f.safe.size.x * f.scale,
                f.safe.size.y * f.scale,
            );
            let r = ui.rect(k);
            let past = r.pos.x < safe.pos.x - 0.5
                || r.pos.y < safe.pos.y - 0.5
                || r.pos.x + r.size.x > safe.pos.x + safe.size.x + 0.5
                || r.pos.y + r.size.y > safe.pos.y + safe.size.y + 0.5;
            if past && seen.insert(sl.node) {
                report.outside_safe.push(sl.node as usize);
            }
        }
    }
    // Controls whose boxes overlap: a click or a tap there could mean either
    // (list rows count once: the same node twice is a pair of rows).
    let slots = ui.slots();
    let controls: Vec<usize> = (0..slots.len())
        .filter(|&k| slots[k].flags & core_ui::flags::FOCUSABLE != 0 && !ui.is_hidden(k))
        .collect();
    let mut pairs = HashSet::new();
    for (n, &a) in controls.iter().enumerate() {
        for &b in &controls[n + 1..] {
            let (sa, sb) = (slots[a], slots[b]);
            if sa.on != sb.on {
                continue;
            }
            let (ra, rb) = (ui.rect(a), ui.rect(b));
            // Touching edges are fine: they must share a whole pixel.
            let w = (ra.pos.x + ra.size.x).min(rb.pos.x + rb.size.x) - ra.pos.x.max(rb.pos.x);
            let h = (ra.pos.y + ra.size.y).min(rb.pos.y + rb.size.y) - ra.pos.y.max(rb.pos.y);
            if w >= 1.0 && h >= 1.0 && pairs.insert((sa.node, sb.node)) {
                report.issues.push(tag(Issue::new(
                    Level::Warning,
                    at(sb.node as usize),
                    format!(
                        "overlaps {}: a click or a tap there could mean either",
                        nodes[sa.node as usize].1.label()
                    ),
                )));
            }
        }
    }
    if let Some(slot) = ui.unreachable(&screen) {
        let i = ui.slots()[slot].node as usize;
        let level = match family {
            Family::N64 | Family::N3ds => Level::Error,
            _ => Level::Warning,
        };
        report.issues.push(tag(Issue::new(
            level,
            at(i),
            "the pad cannot reach this control from the first one".into(),
        )));
    }
    if stats.nodes > target.caps.max_ui_nodes {
        report.issues.push(tag(Issue::new(
            Level::Error,
            None,
            format!(
                "{} nodes laid out: over the target's {} (Caps::max_ui_nodes)",
                stats.nodes, target.caps.max_ui_nodes
            ),
        )));
    }
    if stats.draws > target.caps.max_ui_draws {
        report.issues.push(tag(Issue::new(
            Level::Error,
            None,
            format!(
                "{} draws per frame: over the target's {} (Caps::max_ui_draws)",
                stats.draws, target.caps.max_ui_draws
            ),
        )));
    }
    // Fonts by the paths the screen names them with.
    let mut paths: HashMap<u32, String> = HashMap::new();
    paths.insert(
        FontId::from_path(flat.def.theme.font_path()).0,
        flat.def.theme.font_path().into(),
    );
    for (_, n) in &nodes {
        if !n.font.is_empty() {
            paths.insert(FontId::from_path(&n.font).0, n.font.clone());
        }
    }
    let font_name = |id: u32| {
        paths
            .get(&id)
            .cloned()
            .unwrap_or_else(|| fonts.name(FontId(id)))
    };
    for (font, ch) in probe.missing.iter() {
        report.issues.push(tag(Issue::new(
            Level::Warning,
            None,
            format!("font {} has no glyph for {ch:?}", font_name(*font)),
        )));
    }
    for name in probe.unknown_fonts.iter() {
        report.issues.push(tag(Issue::new(
            Level::Error,
            None,
            format!("font {} is not in the project", font_name(*name)),
        )));
    }
    if let Some(s) = &strings {
        for (i, (_, n)) in nodes.iter().enumerate() {
            if !n.key.is_empty() && !s.has_own(&n.key) {
                let level = if s.get(&n.key).is_some() || !n.text.is_empty() {
                    Level::Warning
                } else {
                    Level::Error
                };
                let msg = if s.get(&n.key).is_some() {
                    format!("key \"{}\" comes from the fallback language", n.key)
                } else {
                    format!("key \"{}\" is in no table; the default text shows", n.key)
                };
                report.issues.push(tag(Issue::new(level, at(i), msg)));
            }
        }
    }
    report
}

/// Sets the screen's preview values on `ui`.
pub fn apply_preview<const N: usize>(ui: &mut Ui<N>, def: &ScreenDef) {
    for (key, v) in &def.preview {
        match v {
            Sample::Int(i) => {
                ui.set(key, *i);
            }
            Sample::Bool(b) => {
                ui.set_bool(key, *b);
            }
            Sample::Text(t) => {
                ui.set_text(key, t);
            }
            Sample::Items(items) => {
                for (k, t) in items.iter().enumerate() {
                    ui.set_item(key, k as u16, core_ui::text_value(t));
                }
            }
        }
    }
    // A list's count follows its items when the preview has no count.
    for (_, n) in def.nodes() {
        if let (Kind::List { .. }, false) = (&n.kind, n.bind.is_empty())
            && !def.preview.contains_key(&n.bind)
        {
            let count = n.children.first().map_or(0, |item| {
                let mut most = 0;
                item.walk(&mut |c| {
                    if let Some(Sample::Items(v)) = def.preview.get(&c.bind) {
                        most = most.max(v.len());
                    }
                });
                most
            });
            ui.set(&n.bind, count as i32);
        }
    }
}

/// Switches `"small"` / `"big"` preview flags with `access.text_scale` on
/// screens that branch on them at 150 % text.
pub fn apply_access_preview<const N: usize>(
    ui: &mut Ui<N>,
    def: &ScreenDef,
    access: core_ui::UiAccess,
) {
    if def
        .nodes()
        .iter()
        .any(|(_, n)| n.visible_if == "big" || n.visible_if == "small")
    {
        let big = access.text_scale >= 1.5;
        ui.set_bool("big", big);
        ui.set_bool("small", !big);
    }
}

/// A renderer that draws nothing: it measures with the project's fonts,
/// notes glyphs they lack and keeps the UI's stats.
pub struct Probe<'f> {
    caps: Caps,
    fonts: &'f Fonts,
    on: u8,
    pub stats: Option<UiStats>,
    /// (font id, character) pairs the fonts lack.
    pub missing: std::collections::BTreeSet<(u32, char)>,
    pub unknown_fonts: std::collections::BTreeSet<u32>,
    /// The smallest line of text drawn, in output pixels (the font's line
    /// height times the draw's scale): Steam Deck's legibility check
    /// (`cargo xtask deck check`, issue #61).
    pub min_text_px: Option<f32>,
}

impl<'f> Probe<'f> {
    pub fn new(caps: Caps, fonts: &'f Fonts) -> Probe<'f> {
        Probe {
            caps,
            fonts,
            on: 0,
            stats: None,
            missing: Default::default(),
            unknown_fonts: Default::default(),
            min_text_px: None,
        }
    }
}

impl Renderer for Probe<'_> {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _clear: Color) {}
    fn draw_sprite(&mut self, _sprite: SpriteId, _pos: Vec2, _params: &SpriteParams) {}
    fn set_camera(&mut self, _camera: &Camera3) {}
    fn draw_model(&mut self, _m: ModelId, _t: &Transform3, _mat: Material, _p: &ModelParams) {}
    fn end_frame(&mut self) {}
    fn draw_text(&mut self, font: FontId, _pos: Vec2, text: &str, params: &TextParams) {
        if !text.trim().is_empty() {
            let h = self.fonts.measure(font, "A", params.scale).y;
            if h > 0.0 {
                self.min_text_px = Some(self.min_text_px.map_or(h, |m: f32| m.min(h)));
            }
        }
        if !self.fonts.knows(font) {
            self.unknown_fonts.insert(font.0);
        } else if let Some(f) = self.fonts.get(font) {
            for ch in text.chars().filter(|c| !c.is_whitespace()) {
                if f.glyph(ch).is_none() {
                    self.missing.insert((font.0, ch));
                }
            }
        }
    }
    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        self.fonts.measure(font, text, scale)
    }
    fn second_screen(&self) -> Option<Vec2> {
        (self.caps.family == Family::N3ds).then_some(Vec2::new(320.0, 240.0))
    }
    fn set_screen(&mut self, screen: u8) {
        if screen == 0 || self.second_screen().is_some() {
            self.on = screen.min(1);
        }
    }
    fn screen(&self) -> Screen {
        if self.on == 1 {
            Screen::Bottom
        } else {
            Screen::Top
        }
    }
    fn ui_stats(&mut self, stats: UiStats) {
        self.stats = Some(stats);
    }
}

/// The screen as `no_std` Rust: `pub static SCREEN: UiScreen<'static>`
/// (`cargo xtask ui export`). Deterministic, marked as generated.
pub fn to_rust(flat: &Flat<'_>) -> String {
    let s = flat.screen();
    let mut out = String::new();
    let _ = writeln!(
        out,
        "// @generated by trino-scene from ui screen `{}` — do not edit.\n\
         // Regenerate: cargo xtask ui export <screen.ui.ron> <this file>\n",
        esc(s.name)
    );
    let _ = writeln!(
        out,
        "#[allow(unused_imports)]\nuse trino_core::ui::{{\n    Align, Anchor, Dir, ImageFit, Justify, NONE, NodeKind, Nodes, Overflow, Scaling, Size,\n    Targets, Theme, UiNode, UiScreen, VAlign,\n}};\n#[allow(unused_imports)]\nuse trino_core::{{Color, FontId, ModelId, Screen, SpriteId, TextAlign}};\n"
    );
    let _ = writeln!(
        out,
        "pub static NODES: [UiNode<'static>; {}] = [",
        flat.nodes.len()
    );
    let d = UiNode::DEFAULT;
    for (i, n) in flat.nodes.iter().enumerate() {
        let _ = writeln!(
            out,
            "    // {i}: {}",
            flat.def
                .root
                .at(&flat.paths[i])
                .map(|n| n.label())
                .unwrap_or_default()
        );
        let _ = writeln!(out, "    UiNode {{");
        let mut field = |name: &str, value: String| {
            let _ = writeln!(out, "        {name}: {value},");
        };
        field("kind", kind_rust(&n.kind));
        if !n.id.is_empty() {
            field("id", format!("\"{}\"", esc(n.id)));
        }
        field("parent", idx(n.parent));
        field("end", n.end.to_string());
        if n.screen != d.screen {
            field("screen", format!("Screen::{:?}", n.screen));
        }
        if n.anchor != d.anchor {
            field("anchor", format!("Anchor::{:?}", n.anchor));
        }
        if n.offset != d.offset {
            field("offset", format!("({}, {})", n.offset.0, n.offset.1));
        }
        if n.size != d.size {
            field(
                "size",
                format!("({}, {})", size_rust(n.size.0), size_rust(n.size.1)),
            );
        }
        if n.min != d.min {
            field("min", format!("({}, {})", n.min.0, n.min.1));
        }
        if n.max != d.max {
            field("max", format!("({}, {})", n.max.0, n.max.1));
        }
        if n.padding != d.padding {
            field("padding", format!("{:?}", n.padding));
        }
        if n.bleed {
            field("bleed", "true".into());
        }
        if n.visible_on != d.visible_on {
            field("visible_on", format!("Targets({})", n.visible_on.0));
        }
        if let Some(c) = n.color {
            field("color", format!("Some({})", color_rust(c)));
        }
        if let Some(f) = n.font {
            let path = flat
                .def
                .root
                .at(&flat.paths[i])
                .map(|d| d.font.clone())
                .unwrap_or_default();
            field("font", format!("Some({})", font_rust(f, &path)));
        }
        if n.text_scale != 0 {
            field("text_scale", n.text_scale.to_string());
        }
        if n.depth != 0 {
            field("depth", n.depth.to_string());
        }
        if n.align != d.align {
            field("align", format!("TextAlign::{:?}", n.align));
        }
        if n.valign != d.valign {
            field("valign", format!("VAlign::{:?}", n.valign));
        }
        if n.overflow != d.overflow {
            field("overflow", format!("Overflow::{:?}", n.overflow));
        }
        for (name, v) in [
            ("text", n.text),
            ("key", n.key),
            ("bind", n.bind),
            ("action", n.action),
            ("visible_if", n.visible_if),
            ("enabled_if", n.enabled_if),
        ] {
            if !v.is_empty() {
                field(name, format!("\"{}\"", esc(v)));
            }
        }
        if n.nav != d.nav {
            let v: Vec<String> = n.nav.iter().map(|&t| idx(t)).collect();
            field("nav", format!("[{}]", v.join(", ")));
        }
        if let Some(icon) = n.cvd_icon {
            let path = flat
                .def
                .root
                .at(&flat.paths[i])
                .map(|d| d.cvd_icon.clone())
                .unwrap_or_default();
            field(
                "cvd_icon",
                format!(
                    "Some(trino_core::ui::CvdIcon {{ sprite: trino_core::SpriteId::from_path(\"{}\"), size: ({}, {}) }})",
                    esc(&path),
                    icon.size.0,
                    icon.size.1
                ),
            );
        }
        let _ = writeln!(out, "        ..UiNode::DEFAULT\n    }},");
    }
    let _ = writeln!(out, "];\n");
    let t = s.theme;
    let dt = core_ui::Theme::DEFAULT;
    let mut theme = String::from("Theme {\n");
    let mut tf = |name: &str, v: String| {
        let _ = writeln!(theme, "        {name}: {v},");
    };
    if t.font != dt.font {
        tf("font", font_rust(t.font, flat.def.theme.font_path()));
    }
    if t.text_scale != dt.text_scale {
        tf("text_scale", t.text_scale.to_string());
    }
    for (name, c, dc) in [
        ("text", t.text, dt.text),
        ("dim", t.dim, dt.dim),
        ("panel", t.panel, dt.panel),
        ("button", t.button, dt.button),
        ("button_text", t.button_text, dt.button_text),
        ("focus", t.focus, dt.focus),
        ("focus_text", t.focus_text, dt.focus_text),
        ("accent", t.accent, dt.accent),
        ("pressed", t.pressed, dt.pressed),
    ] {
        if c != dc {
            tf(name, color_rust(c));
        }
    }
    theme.push_str("        ..Theme::DEFAULT\n    }");
    let theme = if t == dt {
        "Theme::DEFAULT".to_string()
    } else {
        theme
    };
    let _ = writeln!(
        out,
        "pub static SCREEN: UiScreen<'static> = UiScreen {{\n    name: \"{}\",\n    reference: ({}, {}),\n    scaling: Scaling::{:?},\n    wrap: {},\n    focus: {},\n    on_back: \"{}\",\n    theme: {theme},\n    nodes: Nodes::Slice(&NODES),\n}};",
        esc(s.name),
        s.reference.0,
        s.reference.1,
        s.scaling,
        s.wrap,
        idx(s.focus),
        esc(s.on_back),
    );
    out
}

fn kind_rust(k: &NodeKind) -> String {
    match *k {
        NodeKind::Overlay => "NodeKind::Overlay".into(),
        NodeKind::Stack {
            dir,
            gap,
            align,
            justify,
            wrap,
        } => format!(
            "NodeKind::Stack {{ dir: Dir::{dir:?}, gap: {gap}, align: Align::{align:?}, justify: Justify::{justify:?}, wrap: {wrap} }}"
        ),
        NodeKind::Grid { cols, gap } => {
            format!(
                "NodeKind::Grid {{ cols: {cols}, gap: ({}, {}) }}",
                gap.0, gap.1
            )
        }
        NodeKind::Scroll { dir, gap } => {
            format!("NodeKind::Scroll {{ dir: Dir::{dir:?}, gap: {gap} }}")
        }
        NodeKind::List { dir, gap, max } => {
            format!("NodeKind::List {{ dir: Dir::{dir:?}, gap: {gap}, max: {max} }}")
        }
        NodeKind::Text => "NodeKind::Text".into(),
        NodeKind::Image {
            sprite,
            src,
            nine,
            fit,
        } => format!(
            "NodeKind::Image {{ sprite: SpriteId({:#010x}), src: ({}, {}), nine: {nine:?}, fit: ImageFit::{fit:?} }}",
            sprite.0, src.0, src.1
        ),
        NodeKind::Rect {
            border,
            border_color,
        } => format!(
            "NodeKind::Rect {{ border: {border}, border_color: {} }}",
            color_rust(border_color)
        ),
        NodeKind::Button => "NodeKind::Button".into(),
        NodeKind::Toggle => "NodeKind::Toggle".into(),
        NodeKind::Slider { min, max, step } => {
            format!("NodeKind::Slider {{ min: {min}, max: {max}, step: {step} }}")
        }
        NodeKind::Model3d {
            model,
            yaw,
            pitch,
            spin,
            extent,
            lift,
        } => format!(
            "NodeKind::Model3d {{ model: ModelId({:#010x}), yaw: {yaw}, pitch: {pitch}, spin: {spin}, extent: {extent}, lift: {lift} }}",
            model.0
        ),
    }
}

fn size_rust(s: core_ui::Size) -> String {
    match s {
        core_ui::Size::Fit => "Size::Fit".into(),
        core_ui::Size::Px(v) => format!("Size::Px({v})"),
        core_ui::Size::Percent(v) => format!("Size::Percent({v})"),
        core_ui::Size::Fill => "Size::Fill".into(),
    }
}

fn font_rust(id: FontId, path: &str) -> String {
    if !path.is_empty() && FontId::from_path(path) == id {
        format!("FontId::from_path(\"{}\")", esc(path))
    } else {
        format!("FontId({:#010x})", id.0)
    }
}

fn color_rust(c: Color) -> String {
    format!("Color::rgba({}, {}, {}, {})", c.r, c.g, c.b, c.a)
}

fn idx(i: u16) -> String {
    if i == NONE {
        "NONE".into()
    } else {
        i.to_string()
    }
}

fn esc(s: &str) -> String {
    s.replace('\\', "\\\\").replace('"', "\\\"")
}

fn rgba(c: [u8; 4]) -> Color {
    Color::rgba(c[0], c[1], c[2], c[3])
}

fn reference() -> (u16, u16) {
    (320, 240)
}

fn is_reference(r: &(u16, u16)) -> bool {
    *r == reference()
}

fn two() -> u8 {
    2
}

fn eight() -> u16 {
    8
}

fn ten() -> i32 {
    10
}

fn one() -> i32 {
    1
}

fn hundred() -> u16 {
    100
}

fn is_false(b: &bool) -> bool {
    !*b
}

fn is_zero_u8(v: &u8) -> bool {
    *v == 0
}

fn is_zero_i16(v: &i16) -> bool {
    *v == 0
}

fn is_zero_pair(v: &(i16, i16)) -> bool {
    *v == (0, 0)
}

fn is_zero_u16_pair(v: &(u16, u16)) -> bool {
    *v == (0, 0)
}

fn is_zero4(v: &[u8; 4]) -> bool {
    *v == [0; 4]
}

fn is_fit(v: &(Size, Size)) -> bool {
    *v == (Size::Fit, Size::Fit)
}
