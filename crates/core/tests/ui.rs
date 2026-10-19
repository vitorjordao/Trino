//! The UI system (issue #58) through its public API: layout (anchors,
//! stacks, grids, sizes, scaling, safe areas, screens), focus navigation
//! (geometry, overrides, wrap, reachability), pointer and touch, values,
//! lists and scrolling, drawing, and the TUIL / TSTR formats.

mod common;

use trino_core::render3d::Camera3;
use trino_core::ui::tuil::{self, HEADER, RECORD};
use trino_core::ui::{
    Align, Anchor, Dir, Fallback, Justify, NONE, NoStrings, NodeKind, Nodes, Overflow, Scaling,
    Size, StringTable, Targets, Theme, UiNode, UiStrings, UiValue, VAlign, flags, strings,
};
use trino_core::{
    Button, Caps, Color, Family, FontId, InputState, Insets, Key, Material, ModelId, ModelParams,
    PointerButton, Rect, Renderer, Screen, SpriteId, SpriteParams, TextAlign, TextParams,
    Transform3, Ui, UiEnv, UiEvent, UiScreen, UiStats, Vec2,
};

// ------------------------------------------------------------- helpers ----

/// Builds a pre-order node list with parents and subtree ends.
struct Tree {
    nodes: Vec<UiNode<'static>>,
    open: Vec<usize>,
}

impl Tree {
    fn new() -> Self {
        Tree {
            nodes: Vec::new(),
            open: Vec::new(),
        }
    }

    fn open(&mut self, n: UiNode<'static>) -> &mut Self {
        let parent = self.open.last().map_or(NONE, |&p| p as u16);
        self.nodes.push(UiNode { parent, ..n });
        self.open.push(self.nodes.len() - 1);
        self
    }

    fn close(&mut self) -> &mut Self {
        let i = self.open.pop().expect("open node");
        self.nodes[i].end = self.nodes.len() as u16;
        self
    }

    fn leaf(&mut self, n: UiNode<'static>) -> &mut Self {
        self.open(n).close()
    }

    fn screen(&mut self) -> UiScreen<'static> {
        while !self.open.is_empty() {
            self.close();
        }
        let nodes: &'static [UiNode<'static>] = Box::leak(self.nodes.clone().into_boxed_slice());
        let s = UiScreen {
            name: "test",
            reference: (320, 240),
            scaling: Scaling::Integer,
            wrap: false,
            focus: NONE,
            on_back: "close",
            theme: Theme::DEFAULT,
            nodes: Nodes::Slice(nodes),
        };
        s.check_tree().expect("well-formed test tree");
        s
    }
}

fn node(kind: NodeKind) -> UiNode<'static> {
    UiNode {
        kind,
        ..UiNode::DEFAULT
    }
}

fn root() -> UiNode<'static> {
    UiNode {
        size: (Size::Fill, Size::Fill),
        ..node(NodeKind::Overlay)
    }
}

fn boxed(w: u16, h: u16, anchor: Anchor) -> UiNode<'static> {
    UiNode {
        size: (Size::Px(w), Size::Px(h)),
        anchor,
        ..node(NodeKind::Rect {
            border: 0,
            border_color: Color::TRANSPARENT,
        })
    }
}

fn button(id: &'static str, text: &'static str) -> UiNode<'static> {
    UiNode {
        id,
        text,
        action: id,
        size: (Size::Px(80), Size::Px(16)),
        ..node(NodeKind::Button)
    }
}

fn vstack(gap: i16) -> UiNode<'static> {
    node(NodeKind::Stack {
        dir: Dir::Vertical,
        gap,
        align: Align::Start,
        justify: Justify::Start,
    })
}

/// A recording renderer with a monospace font: 6 px per character, 10 px
/// lines (times the scale).
struct Rec {
    caps: Caps,
    bottom: Option<Vec2>,
    on: u8,
    calls: Vec<Call>,
    stats: Vec<UiStats>,
    camera: Option<Camera3>,
    /// A font measured twice as wide (a per-language atlas, #59).
    wide: Option<FontId>,
    /// The font of every `draw_text`.
    fonts: Vec<FontId>,
}

#[derive(Clone, Debug, PartialEq)]
enum Call {
    Fill(Rect, Color, u8),
    Text(String, Vec2, TextParams, u8),
    Sprite(SpriteId, Vec2, SpriteParams),
    Model(ModelId, Transform3),
}

impl Rec {
    fn new(caps: Caps) -> Self {
        Rec {
            caps,
            bottom: (caps.family == Family::N3ds).then_some(Vec2::new(320.0, 240.0)),
            on: 0,
            calls: Vec::new(),
            stats: Vec::new(),
            camera: None,
            wide: None,
            fonts: Vec::new(),
        }
    }

    fn sized(w: u16, h: u16) -> Self {
        Rec::new(Caps {
            screen_width: w,
            screen_height: h,
            ..Caps::PC
        })
    }

    fn fills(&self) -> Vec<(Rect, Color, u8)> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Fill(r, c, s) => Some((*r, *c, *s)),
                _ => None,
            })
            .collect()
    }

    fn texts(&self) -> Vec<(String, Vec2, TextParams, u8)> {
        self.calls
            .iter()
            .filter_map(|c| match c {
                Call::Text(t, p, params, s) => Some((t.clone(), *p, *params, *s)),
                _ => None,
            })
            .collect()
    }
}

impl Renderer for Rec {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _clear: Color) {
        self.calls.clear();
        self.on = 0;
    }
    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.calls.push(Call::Sprite(sprite, pos, *params));
    }
    fn set_camera(&mut self, camera: &Camera3) {
        self.camera = Some(*camera);
    }
    fn draw_model(&mut self, model: ModelId, t: &Transform3, _m: Material, _p: &ModelParams) {
        self.calls.push(Call::Model(model, *t));
    }
    fn end_frame(&mut self) {}
    fn fill_rect(&mut self, rect: Rect, color: Color) {
        self.calls.push(Call::Fill(rect, color, self.on));
    }
    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        self.fonts.push(font);
        self.calls
            .push(Call::Text(text.to_string(), pos, *params, self.on));
    }
    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        let cell = if Some(font) == self.wide { 12.0 } else { 6.0 };
        let w = text
            .split('\n')
            .map(|l| l.chars().count())
            .max()
            .unwrap_or(0);
        let lines = text.split('\n').count();
        Vec2::new(w as f32 * cell * scale, lines as f32 * 10.0 * scale)
    }
    fn second_screen(&self) -> Option<Vec2> {
        self.bottom
    }
    fn set_screen(&mut self, screen: u8) {
        if screen == 1 && self.bottom.is_some() {
            self.on = 1;
        }
    }
    fn screen(&self) -> Screen {
        Screen::from_index(self.on).unwrap_or_default()
    }
    fn ui_stats(&mut self, stats: UiStats) {
        self.stats.push(stats);
    }
}

fn env_of(r: &Rec) -> UiEnv<'static> {
    UiEnv::of(r)
}

fn laid_out(screen: &UiScreen<'_>, r: &Rec) -> Ui<64> {
    let mut ui = Ui::<64>::new();
    let m: &dyn Renderer = r;
    ui.layout(screen, &env_of(r), m);
    ui
}

fn rect_of(ui: &Ui<64>, screen: &UiScreen<'_>, id: &str) -> Rect {
    ui.rect_of(screen, id).expect(id).1
}

fn press(b: Button) -> InputState {
    let mut i = InputState::default();
    i.set(b, true);
    i
}

const IDLE: InputState = InputState::IDLE;

fn tap(ui: &mut Ui<64>, s: &UiScreen<'static>, on: Screen, at: Vec2) -> Vec<String> {
    let mut down = InputState::default();
    down.pointer = Some(at);
    down.pointer_screen = on.index();
    down.set_pointer_button(PointerButton::Primary, true);
    let mut out: Vec<String> = Vec::new();
    for e in ui.update(s, &down, &IDLE, 1.0 / 60.0).iter() {
        out.push(format!("{e:?}"));
    }
    // A touch ends with no pointer at all (3DS): the last place counts.
    let up = InputState::default();
    for e in ui.update(s, &up, &down, 1.0 / 60.0).iter() {
        out.push(format!("{e:?}"));
    }
    out
}

fn clicks(events: &[String]) -> Vec<String> {
    events
        .iter()
        .filter(|e| e.starts_with("Click"))
        .cloned()
        .collect()
}

// -------------------------------------------------------------- layout ----

#[test]
fn nine_anchors_and_stretch_place_children_in_the_root() {
    let mut t = Tree::new();
    t.open(root());
    for (k, a) in Anchor::ALL[..9].iter().enumerate() {
        t.leaf(UiNode {
            id: ["tl", "t", "tr", "l", "c", "r", "bl", "b", "br"][k],
            ..boxed(20, 10, *a)
        });
    }
    t.leaf(UiNode {
        id: "bar",
        offset: (4, 2),
        ..boxed(0, 8, Anchor::StretchBottom)
    });
    t.leaf(UiNode {
        id: "moved",
        offset: (-3, 5),
        ..boxed(10, 10, Anchor::TopRight)
    });
    let s = t.screen();
    let r = Rec::sized(320, 240);
    let ui = laid_out(&s, &r);
    let at = |id| rect_of(&ui, &s, id);
    assert_eq!(at("tl"), Rect::new(0.0, 0.0, 20.0, 10.0));
    assert_eq!(at("t"), Rect::new(150.0, 0.0, 20.0, 10.0));
    assert_eq!(at("tr"), Rect::new(300.0, 0.0, 20.0, 10.0));
    assert_eq!(at("l"), Rect::new(0.0, 115.0, 20.0, 10.0));
    assert_eq!(at("c"), Rect::new(150.0, 115.0, 20.0, 10.0));
    assert_eq!(at("br"), Rect::new(300.0, 230.0, 20.0, 10.0));
    // Stretched: inset by the offset on both sides of that axis.
    assert_eq!(at("bar"), Rect::new(4.0, 234.0, 312.0, 8.0));
    // Offsets are screen axes, whatever the anchor.
    assert_eq!(at("moved"), Rect::new(307.0, 5.0, 10.0, 10.0));
}

#[test]
fn stacks_place_children_in_order_with_gaps_alignment_and_fill() {
    let mut t = Tree::new();
    t.open(root());
    t.open(UiNode {
        id: "col",
        size: (Size::Px(100), Size::Px(100)),
        padding: [2, 4, 2, 4],
        ..node(NodeKind::Stack {
            dir: Dir::Vertical,
            gap: 6,
            align: Align::Center,
            justify: Justify::Start,
        })
    });
    t.leaf(UiNode {
        id: "a",
        ..boxed(40, 10, Anchor::TopLeft)
    });
    t.leaf(UiNode {
        id: "fill",
        size: (Size::Px(20), Size::Fill),
        ..boxed(0, 0, Anchor::TopLeft)
    });
    t.leaf(UiNode {
        id: "b",
        ..boxed(60, 20, Anchor::TopLeft)
    });
    t.close();
    t.open(UiNode {
        id: "row",
        anchor: Anchor::BottomRight,
        size: (Size::Px(120), Size::Px(20)),
        ..node(NodeKind::Stack {
            dir: Dir::Horizontal,
            gap: 0,
            align: Align::Stretch,
            justify: Justify::SpaceBetween,
        })
    });
    for id in ["x", "y", "z"] {
        t.leaf(UiNode {
            id,
            ..boxed(20, 5, Anchor::TopLeft)
        });
    }
    t.close();
    let s = t.screen();
    let ui = laid_out(&s, &Rec::sized(320, 240));
    let at = |id| rect_of(&ui, &s, id);
    // Content box: (2, 4) 96 × 92; 10 + 6 + fill + 6 + 20 = 92 → fill 50.
    assert_eq!(at("a"), Rect::new(30.0, 4.0, 40.0, 10.0));
    assert_eq!(at("fill"), Rect::new(40.0, 20.0, 20.0, 50.0));
    assert_eq!(at("b"), Rect::new(20.0, 76.0, 60.0, 20.0));
    // Space between, stretched to the row's height.
    assert_eq!(at("x"), Rect::new(200.0, 220.0, 20.0, 20.0));
    assert_eq!(at("y"), Rect::new(250.0, 220.0, 20.0, 20.0));
    assert_eq!(at("z"), Rect::new(300.0, 220.0, 20.0, 20.0));
}

#[test]
fn fit_percent_min_max_and_padding_size_nodes() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        id: "label",
        text: "HELLO",
        padding: [3, 1, 3, 1],
        ..node(NodeKind::Text)
    });
    t.leaf(UiNode {
        id: "half",
        anchor: Anchor::Center,
        size: (Size::Percent(50), Size::Percent(25)),
        ..boxed(0, 0, Anchor::Center)
    });
    t.leaf(UiNode {
        id: "clamped",
        anchor: Anchor::BottomLeft,
        size: (Size::Percent(100), Size::Px(4)),
        min: (0, 10),
        max: (90, 0),
        ..boxed(0, 0, Anchor::BottomLeft)
    });
    t.open(UiNode {
        id: "panel",
        anchor: Anchor::Right,
        padding: [5, 5, 5, 5],
        ..node(NodeKind::Rect {
            border: 1,
            border_color: Color::WHITE,
        })
    });
    t.leaf(boxed(30, 12, Anchor::TopLeft));
    t.close();
    let s = t.screen();
    let ui = laid_out(&s, &Rec::sized(320, 240));
    let at = |id| rect_of(&ui, &s, id);
    assert_eq!(at("label"), Rect::new(0.0, 0.0, 36.0, 12.0));
    assert_eq!(at("half"), Rect::new(80.0, 90.0, 160.0, 60.0));
    assert_eq!(at("clamped"), Rect::new(0.0, 230.0, 90.0, 10.0));
    // A Fit panel wraps its children plus padding.
    assert_eq!(at("panel"), Rect::new(280.0, 109.0, 40.0, 22.0));
}

#[test]
fn grids_fill_rows_of_equal_columns() {
    let mut t = Tree::new();
    t.open(root());
    t.open(UiNode {
        size: (Size::Px(100), Size::Fit),
        ..node(NodeKind::Grid {
            cols: 3,
            gap: (5, 2),
        })
    });
    for id in ["g0", "g1", "g2", "g3"] {
        t.leaf(UiNode {
            id,
            ..boxed(10, 8, Anchor::TopLeft)
        });
    }
    t.close();
    let s = t.screen();
    let ui = laid_out(&s, &Rec::sized(320, 240));
    let at = |id| rect_of(&ui, &s, id);
    // (100 - 2 × 5) / 3 = 30 per column.
    assert_eq!(at("g0"), Rect::new(0.0, 0.0, 10.0, 8.0));
    assert_eq!(at("g1"), Rect::new(35.0, 0.0, 10.0, 8.0));
    assert_eq!(at("g2"), Rect::new(70.0, 0.0, 10.0, 8.0));
    assert_eq!(at("g3"), Rect::new(0.0, 10.0, 10.0, 8.0));
}

#[test]
fn integer_scaling_fills_wide_and_tall_screens_and_follows_the_interface_scale() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        id: "corner",
        ..boxed(10, 10, Anchor::BottomRight)
    });
    let s = t.screen();
    let cases = [
        // (screen, interface scale, scaling) → (scale, corner's rect)
        (
            (320, 240),
            1.0,
            Scaling::Integer,
            1.0,
            Rect::new(310.0, 230.0, 10.0, 10.0),
        ),
        (
            (400, 240),
            1.0,
            Scaling::Integer,
            1.0,
            Rect::new(390.0, 230.0, 10.0, 10.0),
        ),
        (
            (1920, 1080),
            1.0,
            Scaling::Integer,
            4.0,
            Rect::new(1880.0, 1040.0, 40.0, 40.0),
        ),
        (
            (3840, 2160),
            1.0,
            Scaling::Integer,
            9.0,
            Rect::new(3750.0, 2070.0, 90.0, 90.0),
        ),
        (
            (2560, 1080),
            1.0,
            Scaling::Integer,
            4.0,
            Rect::new(2520.0, 1040.0, 40.0, 40.0),
        ),
        (
            (1920, 1080),
            1.0,
            Scaling::Fractional,
            4.5,
            Rect::new(1875.0, 1035.0, 45.0, 45.0),
        ),
        (
            (1920, 1080),
            1.5,
            Scaling::Integer,
            6.0,
            Rect::new(1860.0, 1020.0, 60.0, 60.0),
        ),
        // Smaller than the reference: the fit, below 1.
        (
            (160, 120),
            1.0,
            Scaling::Integer,
            0.5,
            Rect::new(155.0, 115.0, 5.0, 5.0),
        ),
    ];
    for ((w, h), iscale, scaling, scale, corner) in cases {
        let r = Rec::sized(w, h);
        let s = UiScreen { scaling, ..s };
        let mut ui = Ui::<64>::new();
        let m: &dyn Renderer = &r;
        ui.layout(&s, &env_of(&r).with_interface_scale(iscale), m);
        assert_eq!(ui.frame(Screen::Top).scale, scale, "{w}x{h} ×{iscale}");
        assert_eq!(rect_of(&ui, &s, "corner"), corner, "{w}x{h} ×{iscale}");
    }
}

/// The PC's stored interface scale (`DisplaySettings::ui_scale`, in
/// quarters) is the factor the layout multiplies its fit by.
#[test]
fn the_display_ui_scale_is_the_interface_scale() {
    use trino_core::DisplaySettings;
    let at = |ui_scale| DisplaySettings {
        ui_scale,
        ..DisplaySettings::DEFAULT
    };
    assert_eq!(at(0).interface_scale(), 1.0, "auto");
    assert_eq!(at(4).interface_scale(), 1.0);
    assert_eq!(at(6).interface_scale(), 1.5);
    assert_eq!(at(8).interface_scale(), 2.0);
    let mut t = Tree::new();
    t.open(root());
    t.leaf(boxed(10, 10, Anchor::Center));
    let s = t.screen();
    let r = Rec::sized(1920, 1080);
    let mut ui = Ui::<8>::new();
    let m: &dyn Renderer = &r;
    ui.layout(
        &s,
        &env_of(&r).with_interface_scale(at(6).interface_scale()),
        m,
    );
    assert_eq!(ui.frame(Screen::Top).scale, 6.0, "4 × 150%");
}

#[test]
fn the_n64_safe_area_insets_the_root_and_bleed_ignores_it() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        id: "bg",
        bleed: true,
        ..boxed(0, 0, Anchor::Fill)
    });
    t.leaf(UiNode {
        id: "hud",
        ..boxed(10, 10, Anchor::TopLeft)
    });
    t.leaf(UiNode {
        id: "end",
        ..boxed(10, 10, Anchor::BottomRight)
    });
    let s = t.screen();
    let r = Rec::new(Caps::N64);
    assert_eq!(r.safe_area(Screen::Top), Insets::symmetric(16, 12));
    assert_eq!(r.safe_area(Screen::Bottom), Insets::ZERO);
    let ui = laid_out(&s, &r);
    assert_eq!(rect_of(&ui, &s, "bg"), Rect::new(0.0, 0.0, 320.0, 240.0));
    assert_eq!(rect_of(&ui, &s, "hud"), Rect::new(16.0, 12.0, 10.0, 10.0));
    assert_eq!(rect_of(&ui, &s, "end"), Rect::new(294.0, 218.0, 10.0, 10.0));
    // The 3DS shows every pixel.
    let ui = laid_out(&s, &Rec::new(Caps::N3DS));
    assert_eq!(rect_of(&ui, &s, "hud"), Rect::new(0.0, 0.0, 10.0, 10.0));
}

#[test]
fn visible_on_and_visible_if_remove_nodes_from_the_layout() {
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(0));
    t.leaf(UiNode {
        id: "pc_only",
        visible_on: Targets::PC.union(Targets::WEB),
        ..boxed(10, 10, Anchor::TopLeft)
    });
    t.leaf(UiNode {
        id: "saved",
        visible_if: "has_save",
        ..boxed(10, 10, Anchor::TopLeft)
    });
    t.leaf(UiNode {
        id: "always",
        ..boxed(10, 10, Anchor::TopLeft)
    });
    let s = t.screen();
    let n64 = Rec::new(Caps::N64);
    let ui = laid_out(&s, &n64);
    assert!(ui.rect_of(&s, "pc_only").is_none());
    assert!(ui.rect_of(&s, "saved").is_none(), "an unset bool hides");
    assert_eq!(rect_of(&ui, &s, "always").pos.y, 12.0);
    let pc = Rec::sized(320, 240);
    let mut ui = laid_out(&s, &pc);
    assert_eq!(rect_of(&ui, &s, "always").pos.y, 10.0);
    ui.set_bool("has_save", true);
    let m: &dyn Renderer = &pc;
    ui.layout(&s, &env_of(&pc), m);
    assert_eq!(rect_of(&ui, &s, "always").pos.y, 20.0);
}

#[test]
fn bottom_screen_nodes_go_to_the_3ds_touch_screen_and_collapse_elsewhere() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        id: "top",
        ..button("top", "TOP")
    });
    t.leaf(UiNode {
        id: "touch",
        screen: Screen::Bottom,
        anchor: Anchor::Center,
        ..button("touch", "TOUCH")
    });
    let s = t.screen();
    let mut r = Rec::new(Caps::N3DS);
    let mut ui = laid_out(&s, &r);
    assert_eq!(
        ui.rect_of(&s, "touch"),
        Some((Screen::Bottom, Rect::new(120.0, 112.0, 80.0, 16.0)))
    );
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let bottom: Vec<_> = r.texts().into_iter().filter(|t| t.3 == 1).collect();
    assert_eq!(bottom.len(), 1, "{:?}", r.texts());
    assert_eq!(bottom[0].0, "TOUCH");
    // One screen: laid out on the main one, by the same anchor.
    let ui = laid_out(&s, &Rec::new(Caps::N64));
    assert_eq!(
        ui.rect_of(&s, "touch"),
        Some((Screen::Top, Rect::new(120.0, 112.0, 80.0, 16.0)))
    );
}

#[test]
fn text_overflow_is_flagged_cut_with_an_ellipsis_or_wrapped() {
    let long = "ONE TWO THREE FOUR";
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(0));
    for (id, overflow, h) in [
        ("visible", Overflow::Visible, 10),
        ("clip", Overflow::Clip, 10),
        ("ellipsis", Overflow::Ellipsis, 10),
        ("wrap", Overflow::Wrap, 40),
        ("wrap_short", Overflow::Wrap, 10),
    ] {
        t.leaf(UiNode {
            id,
            text: long,
            overflow,
            size: (Size::Px(60), Size::Px(h)),
            ..node(NodeKind::Text)
        });
    }
    t.leaf(UiNode {
        id: "fits",
        text: "OK",
        size: (Size::Px(60), Size::Px(10)),
        ..node(NodeKind::Text)
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = laid_out(&s, &r);
    let flags_of = |ui: &Ui<64>, id| ui.slots()[ui.find(&s, id).unwrap()].flags;
    assert!(flags_of(&ui, "visible") & flags::OVERFLOW != 0);
    assert!(flags_of(&ui, "clip") & flags::OVERFLOW != 0);
    assert_eq!(flags_of(&ui, "ellipsis"), flags::TRUNCATED);
    assert_eq!(flags_of(&ui, "wrap"), 0, "four lines of 10 fit 40");
    assert!(flags_of(&ui, "wrap_short") & flags::OVERFLOW != 0);
    assert_eq!(flags_of(&ui, "fits"), 0);
    assert_eq!(ui.stats().overflow, 3);
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let texts: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    // Visible draws it whole, clip cuts it at the last glyph that fits,
    // ellipsis before "...", and wrap draws a line per row (10 glyphs fit).
    assert_eq!(texts.iter().filter(|t| *t == long).count(), 1, "{texts:?}");
    assert!(texts.contains(&"ONE TWO TH".into()), "{texts:?}");
    assert!(texts.contains(&"ONE TWO...".into()), "{texts:?}");
    assert_eq!(
        texts.iter().filter(|t| *t == "ONE TWO").count(),
        2,
        "{texts:?}"
    );
    assert_eq!(
        texts.iter().filter(|t| *t == "THREE FOUR").count(),
        2,
        "{texts:?}"
    );
}

#[test]
fn keys_and_bound_values_make_the_label() {
    struct Pt;
    impl UiStrings for Pt {
        fn get(&self, key: &str) -> Option<&str> {
            (key == "hud.money").then_some("DINHEIRO {} R$")
        }
        fn revision(&self) -> u32 {
            7
        }
    }
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(0));
    t.leaf(UiNode {
        id: "money",
        key: "hud.money",
        text: "MONEY {}",
        bind: "money",
        ..node(NodeKind::Text)
    });
    t.leaf(UiNode {
        id: "name",
        bind: "player",
        ..node(NodeKind::Text)
    });
    t.leaf(UiNode {
        id: "missing",
        key: "no.such.key",
        ..node(NodeKind::Text)
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    ui.set("money", 1200);
    ui.set_text("player", "ANA");
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let texts: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    assert_eq!(texts, ["MONEY 1200", "ANA", "no.such.key"]);
    r.calls.clear();
    let env = env_of(&r).with_strings(&Pt);
    ui.draw(&mut r, &s, &env);
    assert_eq!(r.texts()[0].0, "DINHEIRO 1200 R$");
    // A value change lays out again (the money grew a digit).
    ui.set("money", 12000);
    r.calls.clear();
    ui.draw(&mut r, &s, &env);
    assert_eq!(rect_of(&ui, &s, "money").size.x, 17.0 * 6.0);
}

// --------------------------------------------------------------- focus ----

fn menu() -> UiScreen<'static> {
    let mut t = Tree::new();
    t.open(root());
    t.open(UiNode {
        anchor: Anchor::Center,
        ..vstack(4)
    });
    for id in ["resume", "options", "quit"] {
        t.leaf(button(id, id));
    }
    t.close();
    t.screen()
}

fn step(ui: &mut Ui<64>, s: &UiScreen<'static>, b: Button) -> Vec<String> {
    let now = press(b);
    let mut out: Vec<String> = ui
        .update(s, &now, &IDLE, 1.0 / 60.0)
        .iter()
        .map(|e| format!("{e:?}"))
        .collect();
    out.extend(
        ui.update(s, &IDLE, &now, 1.0 / 60.0)
            .iter()
            .map(|e| format!("{e:?}")),
    );
    out
}

#[test]
fn the_dpad_moves_the_focus_by_geometry_and_a_activates_it() {
    let s = menu();
    let r = Rec::new(Caps::N64);
    let mut ui = laid_out(&s, &r);
    // The first control is focused on the first update.
    ui.update(&s, &IDLE, &IDLE, 0.0);
    assert_eq!(ui.focused(&s), Some(("resume", u16::MAX)));
    let e = step(&mut ui, &s, Button::DpadDown);
    assert_eq!(ui.focused(&s).unwrap().0, "options");
    assert!(e[0].starts_with("Moved"), "{e:?}");
    step(&mut ui, &s, Button::DpadDown);
    assert_eq!(ui.focused(&s).unwrap().0, "quit");
    // No wrap: stays at the end.
    step(&mut ui, &s, Button::DpadDown);
    assert_eq!(ui.focused(&s).unwrap().0, "quit");
    let e = step(&mut ui, &s, Button::A);
    assert_eq!(
        clicks(&e),
        [format!(
            "{:?}",
            UiEvent::Click {
                action: "quit",
                id: "quit",
                item: u16::MAX
            }
        )]
    );
    let e = step(&mut ui, &s, Button::B);
    assert!(e.iter().any(|e| e == "Back { action: \"close\" }"), "{e:?}");
    // With wrap on, down from the last goes to the first.
    let s = UiScreen { wrap: true, ..s };
    step(&mut ui, &s, Button::DpadDown);
    assert_eq!(ui.focused(&s).unwrap().0, "resume");
    assert_eq!(ui.unreachable(&s), None);
}

#[test]
fn a_held_direction_repeats_and_the_sticks_count() {
    let s = menu();
    let mut ui = laid_out(&s, &Rec::new(Caps::N64));
    ui.update(&s, &IDLE, &IDLE, 0.0);
    let down = press(Button::DpadDown);
    ui.update(&s, &down, &IDLE, 0.016);
    assert_eq!(ui.focused(&s).unwrap().0, "options");
    // Held: nothing until the delay, then one step.
    ui.update(&s, &down, &down, 0.3);
    assert_eq!(ui.focused(&s).unwrap().0, "options");
    ui.update(&s, &down, &down, 0.15);
    assert_eq!(ui.focused(&s).unwrap().0, "quit");
    ui.update(&s, &IDLE, &down, 0.016);
    // The N64's C buttons are the second stick.
    let mut c_up = InputState::default();
    c_up.right_stick = Vec2::new(0.0, 1.0);
    ui.update(&s, &c_up, &IDLE, 0.016);
    assert_eq!(ui.focused(&s).unwrap().0, "options");
    let mut stick = InputState::default();
    stick.stick = Vec2::new(0.1, -0.9);
    ui.update(&s, &IDLE, &c_up, 0.016);
    ui.update(&s, &stick, &IDLE, 0.016);
    assert_eq!(ui.focused(&s).unwrap().0, "quit");
}

#[test]
fn grids_navigate_in_two_dimensions_and_overrides_win() {
    let mut t = Tree::new();
    t.open(root());
    t.open(UiNode {
        size: (Size::Px(200), Size::Fit),
        ..node(NodeKind::Grid {
            cols: 2,
            gap: (4, 4),
        })
    });
    for id in ["a", "b", "c", "d"] {
        t.leaf(button(id, id));
    }
    t.close();
    // "e" says: up goes to "b".
    t.leaf(UiNode {
        anchor: Anchor::BottomLeft,
        nav: [3, NONE, NONE, NONE],
        ..button("e", "e")
    });
    let s = t.screen();
    let mut ui = laid_out(&s, &Rec::sized(320, 240));
    ui.update(&s, &IDLE, &IDLE, 0.0);
    let path = [
        (Button::DpadRight, "b"),
        (Button::DpadDown, "d"),
        (Button::DpadLeft, "c"),
        (Button::DpadUp, "a"),
        (Button::DpadDown, "c"),
        (Button::DpadDown, "e"),
        (Button::DpadUp, "b"),
    ];
    for (b, want) in path {
        step(&mut ui, &s, b);
        assert_eq!(ui.focused(&s).unwrap().0, want, "{b:?}");
    }
    assert_eq!(ui.unreachable(&s), None);
}

#[test]
fn unreachable_controls_are_found() {
    let mut t = Tree::new();
    t.open(root());
    // "a" and "b" point at each other; "far" is never a target.
    t.leaf(UiNode {
        nav: [2, 2, 2, 2],
        ..button("a", "a")
    });
    t.leaf(UiNode {
        anchor: Anchor::Left,
        nav: [1, 1, 1, 1],
        ..button("b", "b")
    });
    t.leaf(UiNode {
        anchor: Anchor::BottomRight,
        ..button("far", "far")
    });
    let s = t.screen();
    let ui = laid_out(&s, &Rec::sized(320, 240));
    let missing = ui.unreachable(&s).expect("far is unreachable");
    assert_eq!(s.node(ui.slots()[missing].node as usize).id, "far");
}

#[test]
fn disabled_controls_are_skipped_and_dimmed() {
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(2));
    t.leaf(button("one", "one"));
    t.leaf(UiNode {
        enabled_if: "can_buy",
        ..button("buy", "buy")
    });
    t.leaf(button("three", "three"));
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = laid_out(&s, &r);
    ui.update(&s, &IDLE, &IDLE, 0.0);
    step(&mut ui, &s, Button::DpadDown);
    assert_eq!(ui.focused(&s).unwrap().0, "three");
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let buy = r.texts().into_iter().find(|t| t.0 == "buy").unwrap();
    assert_eq!(buy.2.tint, Theme::DEFAULT.dim);
    ui.set_bool("can_buy", true);
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    step(&mut ui, &s, Button::DpadUp);
    assert_eq!(ui.focused(&s).unwrap().0, "buy");
}

// ------------------------------------------------------ pointer, touch ----

#[test]
fn the_mouse_hovers_and_clicks_and_a_touch_taps_the_bottom_screen() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(button("top", "top"));
    t.leaf(UiNode {
        screen: Screen::Bottom,
        anchor: Anchor::Center,
        ..button("map", "map")
    });
    let s = t.screen();
    let r = Rec::new(Caps::N3DS);
    let mut ui = laid_out(&s, &r);
    ui.update(&s, &IDLE, &IDLE, 0.0);
    assert_eq!(ui.focused(&s).unwrap().0, "top");
    // A tap on the bottom screen: press, then the finger lifts.
    let e = tap(&mut ui, &s, Screen::Bottom, Vec2::new(160.0, 120.0));
    assert_eq!(clicks(&e).len(), 1, "{e:?}");
    assert!(clicks(&e)[0].contains("\"map\""));
    // The same place on the top screen is empty: no click.
    let e = tap(&mut ui, &s, Screen::Top, Vec2::new(160.0, 120.0));
    assert!(clicks(&e).is_empty(), "{e:?}");
    // A mouse moving over a control focuses it.
    let mut hover = InputState::default();
    hover.pointer = Some(Vec2::new(5.0, 5.0));
    ui.update(&s, &hover, &IDLE, 0.0);
    assert_eq!(ui.focused(&s).unwrap().0, "top");
    // Released elsewhere: no click.
    let mut down = hover;
    down.set_pointer_button(PointerButton::Primary, true);
    ui.update(&s, &down, &hover, 0.0);
    let mut away = InputState::default();
    away.pointer = Some(Vec2::new(300.0, 200.0));
    assert!(ui.update(&s, &away, &down, 0.0).clicked().is_none());
}

#[test]
fn toggles_flip_and_sliders_step_by_pad_and_drag() {
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(4));
    t.leaf(UiNode {
        id: "music",
        text: "MUSIC",
        bind: "music_on",
        size: (Size::Px(120), Size::Px(12)),
        ..node(NodeKind::Toggle)
    });
    t.leaf(UiNode {
        id: "vol",
        text: "VOL",
        bind: "volume",
        size: (Size::Px(120), Size::Px(12)),
        ..node(NodeKind::Slider {
            min: 0,
            max: 10,
            step: 2,
        })
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = laid_out(&s, &r);
    ui.set("volume", 4);
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    ui.update(&s, &IDLE, &IDLE, 0.0);
    let e = step(&mut ui, &s, Button::A);
    assert!(
        e.iter()
            .any(|e| e.contains("Changed { bind: \"music_on\", value: 1")),
        "{e:?}"
    );
    assert!(ui.get_bool("music_on"));
    step(&mut ui, &s, Button::DpadLeft);
    assert!(!ui.get_bool("music_on"), "left sets off");
    step(&mut ui, &s, Button::DpadDown);
    step(&mut ui, &s, Button::DpadRight);
    assert_eq!(ui.get("volume"), Some(6));
    for _ in 0..5 {
        step(&mut ui, &s, Button::DpadRight);
    }
    assert_eq!(ui.get("volume"), Some(10), "clamped at max");
    // Dragging: the track starts after the label (3 chars + gap) and
    // takes the rest of 120.
    let row = rect_of(&ui, &s, "vol");
    let track_x = row.pos.x + 18.0 + 4.0;
    let mut down = InputState::default();
    down.pointer = Some(Vec2::new(track_x + 1.0, row.pos.y + 6.0));
    down.set_pointer_button(PointerButton::Primary, true);
    ui.update(&s, &down, &IDLE, 0.0);
    assert_eq!(ui.get("volume"), Some(0));
    let mut mid = down;
    mid.pointer = Some(Vec2::new(row.pos.x + row.size.x - 1.0, row.pos.y + 6.0));
    let e = ui.update(&s, &mid, &down, 0.0);
    assert_eq!(ui.get("volume"), Some(10));
    assert!(e.iter().any(|e| matches!(
        e,
        UiEvent::Changed {
            bind: "volume",
            value: 10,
            ..
        }
    )));
}

// -------------------------------------------------------- lists, scroll ----

#[test]
fn lists_repeat_their_template_per_item_with_item_values() {
    let mut t = Tree::new();
    t.open(root());
    t.open(UiNode {
        id: "list",
        bind: "count",
        ..node(NodeKind::List {
            dir: Dir::Vertical,
            gap: 2,
            max: 8,
        })
    });
    t.leaf(UiNode {
        id: "item",
        bind: "name",
        action: "pick",
        size: (Size::Px(60), Size::Px(10)),
        ..node(NodeKind::Button)
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    ui.set("count", 3);
    for (k, name) in ["SWORD", "SHIELD", "BOW"].iter().enumerate() {
        ui.set_item("name", k as u16, trino_core::ui::text_value(name));
    }
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let items: Vec<_> = ui.slots().iter().filter(|s| s.item != u16::MAX).collect();
    assert_eq!(items.len(), 3);
    assert_eq!(
        ui.rect(ui.slots().iter().position(|s| s.item == 2).unwrap())
            .pos
            .y,
        24.0
    );
    let texts: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    assert_eq!(texts, ["SWORD", "SHIELD", "BOW"]);
    ui.update(&s, &IDLE, &IDLE, 0.0);
    step(&mut ui, &s, Button::DpadDown);
    let e = step(&mut ui, &s, Button::A);
    assert!(
        e.iter()
            .any(|e| e.contains("action: \"pick\"") && e.contains("item: 1")),
        "{e:?}"
    );
}

#[test]
fn scroll_boxes_hide_what_is_outside_and_follow_the_focus() {
    let mut t = Tree::new();
    t.open(root());
    t.open(UiNode {
        id: "scroll",
        size: (Size::Px(100), Size::Px(50)),
        ..node(NodeKind::Scroll {
            dir: Dir::Vertical,
            gap: 0,
        })
    });
    for id in ["s0", "s1", "s2", "s3", "s4", "s5"] {
        t.leaf(button(id, id));
    }
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let visible: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    assert_eq!(visible, ["s0", "s1", "s2"], "16 px rows in a 50 px box");
    ui.update(&s, &IDLE, &IDLE, 0.0);
    for _ in 0..4 {
        step(&mut ui, &s, Button::DpadDown);
    }
    assert_eq!(ui.focused(&s).unwrap().0, "s4");
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let (offset, max) = ui.scroll_of(1).unwrap();
    assert_eq!(max, 96.0 - 50.0);
    assert!(offset > 0.0);
    r.calls.clear();
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let visible: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    assert!(visible.contains(&"s4".to_string()), "{visible:?}");
    assert!(!visible.contains(&"s0".to_string()), "{visible:?}");
    // The wheel scrolls back.
    let mut wheel = InputState::default();
    wheel.pointer = Some(Vec2::new(10.0, 10.0));
    wheel.wheel = 10.0;
    ui.update(&s, &wheel, &IDLE, 0.0);
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    assert_eq!(ui.scroll_of(1).unwrap().0, 0.0);
    assert_eq!(ui.unreachable(&s), None, "clipped rows stay reachable");
}

// ---------------------------------------------------------------- draw ----

#[test]
fn drawing_fills_controls_highlights_the_focus_and_reports_stats() {
    let s = menu();
    let mut r = Rec::new(Caps::N64);
    let mut ui = Ui::<64>::new();
    let env = UiEnv::of(&r);
    ui.draw(&mut r, &s, &env);
    ui.update(&s, &IDLE, &IDLE, 0.0);
    r.calls.clear();
    let env = UiEnv::of(&r);
    ui.draw(&mut r, &s, &env);
    let fills = r.fills();
    let resume = rect_of(&ui, &s, "resume");
    let options = rect_of(&ui, &s, "options");
    assert!(
        fills.contains(&(resume, Theme::DEFAULT.focus, 0)),
        "{fills:?}"
    );
    assert!(fills.contains(&(options, Theme::DEFAULT.button, 0)));
    // The focus ring: four accent fills inside the focused button.
    let ring = fills
        .iter()
        .filter(|f| f.1 == Theme::DEFAULT.accent)
        .count();
    assert_eq!(ring, 4);
    let texts = r.texts();
    let t = texts.iter().find(|t| t.0 == "resume").unwrap();
    assert_eq!(t.2.tint, Theme::DEFAULT.focus_text);
    let stats = *r.stats.last().unwrap();
    assert_eq!(stats.nodes, 5);
    // 3 fills + 4 ring + glyphs of resume/options/quit.
    assert_eq!(stats.draws as usize, 3 + 4 + 6 + 7 + 4);
    // Held A shows the pressed color.
    ui.update(&s, &press(Button::A), &IDLE, 0.0);
    r.calls.clear();
    let env = UiEnv::of(&r);
    ui.draw(&mut r, &s, &env);
    assert!(r.fills().contains(&(resume, Theme::DEFAULT.pressed, 0)));
}

#[test]
fn text_alignment_and_vertical_placement_follow_the_node() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        id: "c",
        text: "AB",
        align: TextAlign::Center,
        valign: VAlign::Middle,
        size: (Size::Px(100), Size::Px(30)),
        ..node(NodeKind::Text)
    });
    t.leaf(UiNode {
        id: "r",
        text: "AB",
        anchor: Anchor::BottomLeft,
        align: TextAlign::Right,
        valign: VAlign::Bottom,
        size: (Size::Px(100), Size::Px(30)),
        ..node(NodeKind::Text)
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let texts = r.texts();
    assert_eq!(texts[0].1, Vec2::new(50.0, 10.0));
    assert_eq!(texts[0].2.align, TextAlign::Center);
    assert_eq!(texts[1].1, Vec2::new(100.0, 230.0));
}

#[test]
fn nine_slice_images_draw_nine_regions_and_plain_images_one() {
    let sprite = SpriteId::from_path("sprites/panel");
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        size: (Size::Px(100), Size::Px(40)),
        ..node(NodeKind::Image {
            sprite,
            src: (24, 24),
            nine: [8, 8, 8, 8],
            fit: trino_core::ui::ImageFit::Stretch,
        })
    });
    t.leaf(UiNode {
        anchor: Anchor::BottomRight,
        size: (Size::Px(50), Size::Px(50)),
        ..node(NodeKind::Image {
            sprite,
            src: (16, 8),
            nine: [0; 4],
            fit: trino_core::ui::ImageFit::Integer,
        })
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let sprites: Vec<_> = r
        .calls
        .iter()
        .filter_map(|c| match c {
            Call::Sprite(_, p, params) => Some((*p, *params)),
            _ => None,
        })
        .collect();
    assert_eq!(sprites.len(), 10);
    // Corners at 1:1, the middle stretched: 100 - 16 = 84 wide from 8.
    assert_eq!(sprites[0].1.region, Some(Rect::new(0.0, 0.0, 8.0, 8.0)));
    assert_eq!(sprites[4].0, Vec2::new(8.0, 8.0));
    assert_eq!(sprites[4].1.scale, Vec2::new(84.0 / 8.0, 24.0 / 8.0));
    assert_eq!(sprites[8].0, Vec2::new(92.0, 32.0));
    // Integer fit: ×3 (48 × 24) centered in 50 × 50.
    assert_eq!(sprites[9].1.scale, Vec2::new(3.0, 3.0));
    assert_eq!(sprites[9].0, Vec2::new(271.0, 203.0));
}

#[test]
fn a_model3d_node_puts_its_model_in_front_of_everything_inside_its_box() {
    let model = ModelId::from_path("models/star");
    let mut t = Tree::new();
    t.open(root());
    t.leaf(UiNode {
        id: "star",
        anchor: Anchor::Right,
        size: (Size::Px(60), Size::Px(60)),
        ..node(NodeKind::Model3d {
            model,
            yaw: 30,
            pitch: 0,
            spin: 90,
            extent: 200,
            lift: 0,
        })
    });
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    ui.update(&s, &IDLE, &IDLE, 1.0);
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    let cam = r.camera.expect("a camera for the box");
    let Call::Model(id, tr) = r
        .calls
        .iter()
        .find(|c| matches!(c, Call::Model(..)))
        .unwrap()
    else {
        unreachable!()
    };
    assert_eq!(*id, model);
    // Project the model's origin with the engine's T&L rule.
    let half = cam.fov_y * 0.5;
    let focal = half.cos() / half.sin() * 240.0 * 0.5;
    let sx = 160.0 + tr.position.x * focal / tr.position.z;
    let sy = 120.0 - tr.position.y * focal / tr.position.z;
    let rect = rect_of(&ui, &s, "star");
    // (The engine's sin/cos are approximations: within a fifth of a pixel.)
    assert!((sx - (rect.pos.x + 30.0)).abs() < 0.2, "{sx}");
    assert!((sy - (rect.pos.y + 30.0)).abs() < 0.2, "{sy}");
    // Two units (extent 200) span the box's 60 px; spin turned it 90°.
    assert!((2.0 * tr.scale.x * focal / tr.position.z - 60.0).abs() < 0.2);
    assert!((tr.rotation.y - 120f32.to_radians()).abs() < 1e-4);
    assert!(tr.position.z < 0.5, "just past the near plane");
}

#[test]
fn draw_on_lets_a_game_draw_the_bottom_screen_itself() {
    let mut t = Tree::new();
    t.open(root());
    t.leaf(button("top", "top"));
    t.leaf(UiNode {
        screen: Screen::Bottom,
        ..button("bottom", "bottom")
    });
    let s = t.screen();
    let mut r = Rec::new(Caps::N3DS);
    let mut ui = Ui::<64>::new();
    let env = UiEnv::of(&r);
    ui.draw_on(&mut r, &s, &env, Screen::Top);
    assert_eq!(r.texts().len(), 1);
    assert!(r.on_screen(Screen::Bottom));
    ui.draw_on(&mut r, &s, &env, Screen::Bottom);
    let t: Vec<_> = r.texts().into_iter().map(|t| (t.0, t.3)).collect();
    assert_eq!(t, [("top".to_string(), 0), ("bottom".to_string(), 1)]);
}

#[test]
fn a_ui_smaller_than_its_screen_drops_nodes_and_says_so() {
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(0));
    for _ in 0..10 {
        t.leaf(boxed(4, 4, Anchor::TopLeft));
    }
    let s = t.screen();
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<4>::new();
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    assert_eq!(ui.slots().len(), 4);
    assert_eq!(ui.stats().dropped, 8);
}

// ------------------------------------------------------------- formats ----

fn bake(screen: &UiScreen<'_>) -> Vec<u8> {
    // Two passes: collect the strings, then encode against the final pool.
    let mut pool = String::new();
    {
        let mut add = |s: &str| {
            if !s.is_empty() && !pool.contains(s) {
                pool.push_str(s);
            }
            (0, 0)
        };
        tuil::encode_header(screen, screen.len(), 0, &mut add);
        for i in 0..screen.len() {
            tuil::encode_record(&screen.node(i), &mut add);
        }
    }
    let mut find = |s: &str| match s {
        "" => (0, 0),
        s => (pool.find(s).unwrap() as u16, s.len() as u16),
    };
    let mut out = tuil::encode_header(screen, screen.len(), pool.len(), &mut find).to_vec();
    for i in 0..screen.len() {
        out.extend_from_slice(&tuil::encode_record(&screen.node(i), &mut find));
    }
    out.extend_from_slice(pool.as_bytes());
    out
}

#[test]
fn tuil_round_trips_every_node_kind_and_field() {
    let mut t = Tree::new();
    t.open(UiNode {
        id: "root",
        size: (Size::Fill, Size::Percent(90)),
        padding: [1, 2, 3, 4],
        ..root()
    });
    t.open(UiNode {
        id: "stack",
        offset: (-5, 7),
        anchor: Anchor::StretchCenter,
        visible_on: Targets::N64.union(Targets::N3DS),
        ..node(NodeKind::Stack {
            dir: Dir::Horizontal,
            gap: -3,
            align: Align::End,
            justify: Justify::SpaceBetween,
        })
    });
    t.leaf(UiNode {
        id: "label",
        text: "Olá {}",
        key: "hud.hello",
        bind: "name",
        color: Some(Color::rgba(1, 2, 3, 4)),
        font: Some(FontId::from_path("fonts/big")),
        text_scale: 2,
        align: TextAlign::Right,
        valign: VAlign::Bottom,
        overflow: Overflow::Wrap,
        min: (3, 4),
        max: (300, 200),
        ..node(NodeKind::Text)
    });
    t.leaf(UiNode {
        screen: Screen::Bottom,
        bleed: true,
        visible_if: "shown",
        enabled_if: "on",
        nav: [0, 1, 2, NONE],
        ..button("go", "Go")
    });
    t.close();
    t.leaf(node(NodeKind::Grid {
        cols: 4,
        gap: (2, -1),
    }));
    t.leaf(node(NodeKind::Scroll {
        dir: Dir::Horizontal,
        gap: 9,
    }));
    t.leaf(node(NodeKind::List {
        dir: Dir::Vertical,
        gap: 1,
        max: 12,
    }));
    t.leaf(node(NodeKind::Image {
        sprite: SpriteId(0xdead_beef),
        src: (17, 9),
        nine: [1, 2, 3, 4],
        fit: trino_core::ui::ImageFit::Contain,
    }));
    t.leaf(node(NodeKind::Rect {
        border: 3,
        border_color: Color::rgb(9, 8, 7),
    }));
    t.leaf(node(NodeKind::Toggle));
    t.leaf(node(NodeKind::Slider {
        min: -100,
        max: 100_000,
        step: 7,
    }));
    t.leaf(node(NodeKind::Model3d {
        model: ModelId(42),
        yaw: -45,
        pitch: 10,
        spin: -90,
        extent: 150,
        lift: -25,
    }));
    let s = UiScreen {
        name: "every",
        reference: (400, 240),
        scaling: Scaling::Fractional,
        wrap: true,
        focus: 3,
        on_back: "back",
        theme: Theme {
            accent: Color::rgb(1, 1, 1),
            text_scale: 2,
            ..Theme::DEFAULT
        },
        ..t.screen()
    };
    let blob = bake(&s);
    assert!(blob.len() > HEADER + s.len() * RECORD);
    let back = UiScreen::parse(&blob).expect("parses");
    assert_eq!(back.name, "every");
    assert_eq!(back.reference, (400, 240));
    assert_eq!(back.scaling, Scaling::Fractional);
    assert!(back.wrap);
    assert_eq!(back.focus, 3);
    assert_eq!(back.on_back, "back");
    assert_eq!(back.theme, s.theme);
    assert_eq!(back.len(), s.len());
    for i in 0..s.len() {
        assert_eq!(back.node(i), s.node(i), "node {i}");
    }
}

#[test]
fn tuil_refuses_damaged_blobs() {
    let blob = bake(&menu());
    assert!(UiScreen::parse(&blob).is_ok());
    let mut bad = blob.clone();
    bad[0] = b'X';
    assert_eq!(
        UiScreen::parse(&bad).unwrap_err(),
        trino_core::ui::UiError::BadMagic
    );
    let mut bad = blob.clone();
    bad[4] = 9;
    assert!(matches!(
        UiScreen::parse(&bad),
        Err(trino_core::ui::UiError::UnsupportedVersion(9))
    ));
    assert_eq!(
        UiScreen::parse(&blob[..blob.len() - 1]).unwrap_err(),
        trino_core::ui::UiError::Truncated
    );
    // A child whose subtree runs past its parent's.
    let mut bad = blob.clone();
    let rec = HEADER + RECORD * 2;
    bad[rec + 4] = 99;
    assert_eq!(
        UiScreen::parse(&bad).unwrap_err(),
        trino_core::ui::UiError::BadTree
    );
    // A string past the pool.
    let mut bad = blob.clone();
    bad[HEADER + 52] = 200;
    assert_eq!(
        UiScreen::parse(&bad).unwrap_err(),
        trino_core::ui::UiError::BadString
    );
}

#[test]
fn a_baked_screen_lays_out_like_its_static_twin() {
    let s = menu();
    let blob = bake(&s);
    let baked = UiScreen::parse(&blob).unwrap();
    let r = Rec::new(Caps::N64);
    let a = laid_out(&s, &r);
    let b = laid_out(&baked, &r);
    assert_eq!(a.slots(), b.slots());
    for i in 0..a.slots().len() {
        assert_eq!(a.rect(i), b.rect(i));
    }
}

#[test]
fn string_tables_bake_look_up_and_fall_back() {
    let mut bytes = Vec::new();
    let mut entries = [
        ("menu.play", "Jogar"),
        ("menu.quit", "Sair"),
        ("ç", "cedilha"),
    ];
    strings::encode_tstr("pt-BR", "en-US", &mut entries, &mut |b| {
        bytes.extend_from_slice(b)
    })
    .unwrap();
    let pt = StringTable::parse(&bytes).unwrap();
    assert_eq!(pt.lang(), "pt-BR");
    assert_eq!(pt.fallback(), "en-US");
    assert_eq!(pt.len(), 3);
    assert_eq!(pt.lookup("menu.play"), Some("Jogar"));
    assert_eq!(pt.lookup("ç"), Some("cedilha"));
    assert_eq!(pt.lookup("menu.options"), None);
    let mut en_bytes = Vec::new();
    let mut en = [("menu.options", "Options"), ("menu.play", "Play")];
    strings::encode_tstr("en-US", "", &mut en, &mut |b| en_bytes.extend_from_slice(b)).unwrap();
    let en = StringTable::parse(&en_bytes).unwrap();
    let both = Fallback {
        first: pt,
        then: Some(en),
    };
    assert_eq!(both.get("menu.play"), Some("Jogar"));
    assert_eq!(both.get("menu.options"), Some("Options"));
    assert_ne!(both.revision(), en.revision());
    assert_eq!(NoStrings.get("menu.play"), None);
    // Damage.
    assert_eq!(
        StringTable::parse(&bytes[..10]).unwrap_err(),
        trino_core::ui::StringsError::Truncated
    );
    let mut bad = bytes.clone();
    bad[1] = 0;
    assert_eq!(
        StringTable::parse(&bad).unwrap_err(),
        trino_core::ui::StringsError::BadMagic
    );
}

#[test]
fn values_keep_ints_bools_texts_per_item_and_refuse_when_full() {
    let mut ui = Ui::<8>::new();
    assert!(ui.set("a", 1));
    assert_eq!(ui.get("a"), Some(1));
    let rev = ui.values().revision();
    ui.set("a", 1);
    assert_eq!(ui.values().revision(), rev, "same value: no change");
    ui.set_bool("b", true);
    assert_eq!(ui.get("b"), Some(1));
    ui.set_text(
        "c",
        "a very long name that does not fit in thirty-two bytes",
    );
    let Some(UiValue::Text(t)) = ui.values().get("c", u16::MAX) else {
        panic!()
    };
    assert_eq!(t.as_str().len(), 32);
    ui.set_item("d", 2, UiValue::Int(5));
    assert_eq!(ui.values().get("d", 2), Some(UiValue::Int(5)));
    assert_eq!(ui.values().get("d", 3), None);
    // An item reads the screen's value when it has none.
    assert_eq!(ui.values().get("a", 3), Some(UiValue::Int(1)));
    for k in 0..30 {
        ui.set(&format!("k{k}"), k);
    }
    assert!(!ui.set("overflowing", 1));
    assert!(ui.values().dropped > 0);
    ui.values_mut().clear();
    assert!(ui.values().is_empty());
}

#[test]
fn keyboard_enter_and_escape_work_like_a_and_b() {
    let s = menu();
    let mut ui = laid_out(&s, &Rec::sized(320, 240));
    ui.update(&s, &IDLE, &IDLE, 0.0);
    let mut enter = InputState::default();
    enter.set_key(Key::Enter, true);
    assert_eq!(ui.update(&s, &enter, &IDLE, 0.0).clicked(), Some("resume"));
    let mut esc = InputState::default();
    esc.set_key(Key::Escape, true);
    assert!(ui.update(&s, &esc, &IDLE, 0.0).back());
}

#[test]
fn every_caps_table_names_its_family_and_ui_budget() {
    for f in Family::ALL {
        let caps = f.caps();
        assert_eq!(caps.family, f);
        assert!(caps.max_ui_nodes >= 64 && caps.max_ui_draws >= 256, "{f:?}");
    }
    assert_eq!(Caps::N64_EXPANDED.family, Family::N64);
    assert_eq!(Caps::N3DS_NEW.family, Family::N3ds);
    assert_eq!(Caps::N64.safe_area, (16, 12));
    let env = UiEnv::of_caps(&Caps::N3DS);
    assert_eq!(env.bottom, Some(Vec2::new(320.0, 240.0)));
    assert_eq!(
        UiEnv::of_caps(&Caps::N64).safe[0],
        Insets::symmetric(16, 12)
    );
}

#[test]
fn baked_blobs_are_parsed_once_and_again_when_the_platform_swaps_them() {
    use trino_core::BlobId;
    use trino_core::ui::{ScreenBlob, StringsBlob};
    let mut platform = common::MemoryPlatform::new(0);
    let mut pause = ScreenBlob::new("pause");
    assert!(pause.get(&platform).is_none(), "no blob yet");
    platform.insert_blob(BlobId::from_path("ui/pause"), bake(&menu()));
    assert_eq!(pause.get(&platform).map(|s| s.len()), Some(5));
    // A live reload hands out new bytes: the screen follows.
    let mut t = Tree::new();
    t.open(root());
    t.leaf(button("only", "only"));
    platform.insert_blob(BlobId::from_path("ui/pause"), bake(&t.screen()));
    assert_eq!(pause.get(&platform).map(|s| s.len()), Some(2));
    assert_eq!(pause.screen().unwrap().node(1).id, "only");
    // Broken bytes: no screen, and why.
    platform.insert_blob(BlobId::from_path("ui/pause"), b"TUIL".to_vec());
    assert!(pause.get(&platform).is_none());
    assert_eq!(pause.error, Some(trino_core::ui::UiError::Truncated));
    let mut bytes = Vec::new();
    let mut entries = [("a", "b")];
    strings::encode_tstr("pt-BR", "", &mut entries, &mut |b| {
        bytes.extend_from_slice(b)
    })
    .unwrap();
    platform.insert_blob(BlobId::from_path("i18n/pt-BR"), bytes);
    let mut pt = StringsBlob::new("pt-BR");
    assert_eq!(pt.get(&platform).and_then(|t| t.lookup("a")), Some("b"));
}

#[test]
fn menus_work_before_their_first_draw() {
    // No layout, no draw: update lays the screen out at its reference size
    // with the engine font's cell (headless tests, update before render).
    let s = menu();
    let mut ui = Ui::<64>::new();
    ui.update(&s, &IDLE, &IDLE, 0.0);
    assert_eq!(ui.focused(&s).unwrap().0, "resume");
    step(&mut ui, &s, Button::DpadDown);
    let e = step(&mut ui, &s, Button::A);
    assert_eq!(clicks(&e).len(), 1);
    assert!(clicks(&e)[0].contains("\"options\""), "{e:?}");
    // A draw lays it out for the real target, keeping the focus; after a
    // reset, update lays out for that target again.
    let mut r = Rec::new(Caps::N64);
    let env = env_of(&r);
    ui.draw(&mut r, &s, &env);
    assert_eq!(ui.focused(&s).unwrap().0, "options");
    assert_eq!(rect_of(&ui, &s, "resume").pos.x, 120.0);
    ui.reset();
    ui.update(&s, &IDLE, &IDLE, 0.0);
    assert_eq!(ui.frame(Screen::Top).safe.pos, Vec2::new(16.0, 12.0));
}

/// A language that draws the theme's font with its own atlas (issue #59):
/// the layout measures with it and the draw uses it, so overflow checks and
/// the pixels agree.
#[test]
fn a_languages_font_remap_reaches_layout_and_draw() {
    use trino_core::i18n::table::{EntrySpec, TableSpec, encode_tstr_with};
    let base = Theme::DEFAULT.font;
    let pt_font = FontId::from_path("fonts/trino5x7@pt-BR");
    let remap = [(base.0, pt_font.0)];
    let spec = TableSpec {
        fonts: &remap,
        ..TableSpec::plain("pt-BR", "")
    };
    let mut entries = [("title", EntrySpec::Text("Olá"))];
    let mut bytes = Vec::new();
    encode_tstr_with(&spec, &mut entries, &mut |b| bytes.extend_from_slice(b)).unwrap();
    let pt = StringTable::parse(&bytes).unwrap();
    let screen = Tree::new()
        .open(root())
        .leaf(UiNode {
            id: "t",
            key: "title",
            text: "Hi",
            ..node(NodeKind::Text)
        })
        .screen();
    let mut r = Rec::sized(320, 240);
    r.wide = Some(pt_font);
    let env = UiEnv::of(&r).with_strings(&pt);
    let mut ui = Ui::<64>::new();
    ui.layout(&screen, &env, &r as &dyn Renderer);
    assert_eq!(rect_of(&ui, &screen, "t").size.x, 36.0, "3 chars at 12 px");
    ui.draw(&mut r, &screen, &env);
    assert_eq!(r.fonts, vec![pt_font]);
    // Without the table: the theme's font, the node's own text.
    let mut ui = Ui::<64>::new();
    ui.layout(&screen, &UiEnv::of(&r), &r as &dyn Renderer);
    assert_eq!(rect_of(&ui, &screen, "t").size.x, 12.0);
}

// ------------------------------------------------- game data (#95) ----

/// A squad far past `MAX_VALUES`, answered by the game on demand.
struct Squad {
    names: Vec<String>,
    lookups: std::cell::Cell<u32>,
    revision: Option<u32>,
}

impl trino_core::ui::UiData for Squad {
    fn value(&self, key: &str, item: u16) -> Option<trino_core::ui::UiValue> {
        self.lookups.set(self.lookups.get() + 1);
        match key {
            "count" => Some(trino_core::ui::UiValue::Int(self.names.len() as i32)),
            "name" => self
                .names
                .get(item as usize)
                .map(|n| trino_core::ui::text_value(n)),
            "star" => Some(trino_core::ui::UiValue::Bool(item.is_multiple_of(3))),
            "title" => Some(trino_core::ui::text_value("SQUAD")),
            _ => None,
        }
    }

    fn revision(&self) -> Option<u32> {
        self.revision
    }
}

fn squad_screen() -> UiScreen<'static> {
    let mut t = Tree::new();
    t.open(root());
    t.open(vstack(0));
    t.leaf(UiNode {
        id: "title",
        bind: "title",
        ..node(NodeKind::Text)
    });
    t.open(UiNode {
        id: "scroll",
        size: (Size::Px(100), Size::Px(50)),
        ..node(NodeKind::Scroll {
            dir: Dir::Vertical,
            gap: 0,
        })
    });
    t.open(UiNode {
        id: "list",
        bind: "count",
        ..node(NodeKind::List {
            dir: Dir::Vertical,
            gap: 0,
            max: 0,
        })
    });
    t.open(UiNode {
        id: "row",
        bind: "name",
        action: "pick",
        size: (Size::Px(80), Size::Px(10)),
        ..node(NodeKind::Button)
    });
    t.leaf(UiNode {
        id: "star",
        text: "*",
        visible_if: "star",
        anchor: Anchor::Right,
        ..node(NodeKind::Text)
    });
    t.screen()
}

#[test]
fn game_data_fills_lists_past_the_value_store_and_the_ui_keeps_precedence() {
    let s = squad_screen();
    let names: Vec<String> = (0..40).map(|i| format!("P{i}")).collect();
    let data = Squad {
        names: names.clone(),
        lookups: std::cell::Cell::new(0),
        revision: None,
    };
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<128>::new();
    let env = env_of(&r).with_data(&data);
    ui.draw(&mut r, &s, &env);
    // 40 rows (more than the 24 values a Ui keeps), item values and
    // per-item `visible_if` from the game; the 50 px box shows five.
    let rows = ui
        .slots()
        .iter()
        .filter(|sl| sl.item != trino_core::ui::NO_ITEM && s.node(sl.node as usize).id == "row")
        .count();
    assert_eq!(rows, 40);
    let texts: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    assert_eq!(texts, ["SQUAD", "P0", "*", "P1", "P2", "P3", "*", "P4"]);
    assert_eq!(ui.values().len(), 0, "nothing copied into the Ui");
    // A value the Ui holds wins over the game's.
    ui.set_text("title", "MINE");
    r.calls.clear();
    ui.draw(&mut r, &s, &env);
    assert_eq!(r.texts()[0].0, "MINE");
}

#[test]
fn game_data_without_a_revision_lays_out_every_draw_and_with_one_only_on_change() {
    let s = squad_screen();
    let mut data = Squad {
        names: vec!["A".into(), "B".into()],
        lookups: std::cell::Cell::new(0),
        revision: None,
    };
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<64>::new();
    let base = env_of(&r);
    ui.draw(&mut r, &s, &base.with_data(&data));
    // No revision: a change shows on the very next draw.
    data.names.push("C".into());
    r.calls.clear();
    let base = env_of(&r);
    ui.draw(&mut r, &s, &base.with_data(&data));
    assert!(r.texts().iter().any(|t| t.0 == "C"));
    // A revision: no layout (no lookups) while it stays.
    data.revision = Some(7);
    let base = env_of(&r);
    ui.draw(&mut r, &s, &base.with_data(&data));
    data.lookups.set(0);
    let before = data.lookups.get();
    data.names.push("D".into());
    r.calls.clear();
    let base = env_of(&r);
    ui.draw(&mut r, &s, &base.with_data(&data));
    let layout_lookups = data.lookups.get() - before;
    assert!(
        !r.texts().iter().any(|t| t.0 == "D"),
        "the same revision keeps the layout"
    );
    data.revision = Some(8);
    data.lookups.set(0);
    r.calls.clear();
    let base = env_of(&r);
    ui.draw(&mut r, &s, &base.with_data(&data));
    assert!(
        r.texts().iter().any(|t| t.0 == "D"),
        "a new revision lays out"
    );
    assert!(data.lookups.get() > layout_lookups);
}

#[test]
fn a_cursor_the_game_keeps_focuses_a_list_row_scrolls_it_in_and_gives_its_rect() {
    let s = squad_screen();
    let data = Squad {
        names: (0..20).map(|i| format!("P{i}")).collect(),
        lookups: std::cell::Cell::new(0),
        revision: None,
    };
    let mut r = Rec::sized(320, 240);
    let mut ui = Ui::<128>::new();
    // Before any layout: the next draw places the focus and reveals it.
    assert!(ui.set_focus_item(&s, "row", 12));
    assert!(!ui.set_focus_item(&s, "nope", 0));
    let env = env_of(&r).with_data(&data);
    ui.draw(&mut r, &s, &env);
    assert_eq!(ui.focused(&s), Some(("row", 12)));
    let texts: Vec<String> = r.texts().into_iter().map(|t| t.0).collect();
    assert!(texts.contains(&"P12".to_string()), "scrolled in: {texts:?}");
    assert!(!texts.contains(&"P0".to_string()), "{texts:?}");
    // Its rect is on screen, inside the scroll box; a hidden row has none.
    let (on, row) = ui.rect_of_item(&s, "row", 12).expect("visible row");
    assert_eq!(on, Screen::Top);
    let view = ui.rect_of(&s, "scroll").unwrap().1;
    assert!(row.pos.y >= view.pos.y && row.pos.y + row.size.y <= view.pos.y + view.size.y);
    assert_eq!(ui.rect_of_item(&s, "row", 0), None, "scrolled away");
    assert!(
        ui.rect_of_item(&s, "title", 3).is_some(),
        "outside a list: its one copy"
    );
    // Back to the top: the first rows show again on the same draw.
    ui.set_focus_item(&s, "row", 0);
    r.calls.clear();
    ui.draw(&mut r, &s, &env);
    assert_eq!(r.texts()[1].0, "P0");
    assert_eq!(ui.scroll_of(3).unwrap().0, 0.0);
}
