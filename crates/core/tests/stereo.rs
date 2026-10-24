//! Stereoscopic 3D through the public API (issue #63): the UI's layer
//! depths (inherited, set only where they change), their TUIL v3 record
//! (v2 still loads), and a backend's record-and-replay of a frame.

use trino_core::stereo::{self, Eye, LayerDepth, StereoCmd, StereoData, StereoFrame, StereoMono};
use trino_core::ui::{NONE, NodeKind, Nodes, Scaling, Size, Theme, UiNode, tuil};
use trino_core::{
    Caps, Color, FontId, Material, ModelId, ModelParams, Rect, Renderer, SpriteId, SpriteParams,
    StereoSettings, TextParams, Transform3, Ui, UiEnv, UiScreen, Vec2,
};

/// Every 2D call with the layer depth in effect.
#[derive(Default)]
struct Rec {
    depth: i8,
    depth_calls: Vec<i8>,
    fills: Vec<(Rect, i8)>,
}

impl Renderer for Rec {
    fn caps(&self) -> &Caps {
        &Caps::N3DS
    }
    fn begin_frame(&mut self, _: Color) {}
    fn end_frame(&mut self) {}
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
    fn fill_rect(&mut self, r: Rect, _: Color) {
        self.fills.push((r, self.depth));
    }
    fn set_layer_depth(&mut self, d: LayerDepth) {
        self.depth = d.0;
        self.depth_calls.push(d.0);
    }
}

fn screen(nodes: &'static [UiNode<'static>]) -> UiScreen<'static> {
    let s = UiScreen {
        name: "stereo",
        reference: (320, 240),
        scaling: Scaling::Integer,
        wrap: false,
        focus: NONE,
        on_back: "",
        theme: Theme::DEFAULT,
        nodes: Nodes::Slice(nodes),
    };
    s.check_tree().unwrap();
    s
}

const fn rect(parent: u16, end: u16, x: i16, depth: i8) -> UiNode<'static> {
    UiNode {
        kind: NodeKind::Rect {
            border: 0,
            border_color: Color::BLACK,
        },
        parent,
        end,
        offset: (x, 0),
        size: (Size::Px(10), Size::Px(10)),
        color: Some(Color::WHITE),
        depth,
        ..UiNode::DEFAULT
    }
}

/// root ─┬─ hud (depth 8) ─┬─ a (inherits 8)
///       │                 └─ b (depth -4)
///       └─ c (the screen's plane)
static NODES: [UiNode<'static>; 5] = [
    UiNode {
        size: (Size::Fill, Size::Fill),
        end: 5,
        ..UiNode::DEFAULT
    },
    UiNode {
        kind: NodeKind::Overlay,
        parent: 0,
        end: 4,
        size: (Size::Fill, Size::Fill),
        depth: 8,
        ..UiNode::DEFAULT
    },
    rect(1, 3, 0, 0),
    rect(1, 4, 20, -4),
    rect(0, 5, 40, 0),
];

static FLAT: [UiNode<'static>; 2] = [
    UiNode {
        size: (Size::Fill, Size::Fill),
        end: 2,
        ..UiNode::DEFAULT
    },
    rect(0, 2, 0, 0),
];

#[test]
fn ui_nodes_float_at_their_depth_and_children_inherit_it() {
    let s = screen(&NODES);
    let mut r = Rec::default();
    let mut ui: Ui<16> = Ui::new();
    let env = UiEnv::of(&r);
    ui.draw(&mut r, &s, &env);
    let depths: Vec<(f32, i8)> = r.fills.iter().map(|(f, d)| (f.pos.x, *d)).collect();
    assert_eq!(depths, [(0.0, 8), (20.0, -4), (40.0, 0)]);
    // Set where it changes, and back to the screen's plane at the end.
    assert_eq!(r.depth_calls, [8, -4, 0]);
    assert_eq!(r.depth, 0);
}

#[test]
fn a_screen_without_depths_makes_no_depth_call() {
    let s = screen(&FLAT);
    let mut r = Rec::default();
    let mut ui: Ui<8> = Ui::new();
    let env = UiEnv::of(&r);
    ui.draw(&mut r, &s, &env);
    assert_eq!(r.fills.len(), 1);
    assert!(r.depth_calls.is_empty());
}

/// TUIL v3 carries each node's depth; a v2 blob (102-byte records) still
/// loads, every depth 0.
#[test]
fn tuil_v3_carries_depths_and_v2_still_loads() {
    let s = screen(&NODES);
    let mut pool = String::new();
    let mut refs = std::collections::HashMap::new();
    for i in 0..s.len() {
        let n = s.node(i);
        for t in [
            n.id,
            n.text,
            n.key,
            n.bind,
            n.action,
            n.visible_if,
            n.enabled_if,
            s.name,
            s.on_back,
        ] {
            if !refs.contains_key(t) {
                refs.insert(t.to_string(), (pool.len() as u16, t.len() as u16));
                pool.push_str(t);
            }
        }
    }
    let mut find = |t: &str| refs[t];
    let mut v3 = tuil::encode_header(&s, s.len(), pool.len(), &mut find).to_vec();
    let records: Vec<_> = (0..s.len())
        .map(|i| tuil::encode_record(&s.node(i), &mut find))
        .collect();
    for r in &records {
        v3.extend_from_slice(r);
    }
    v3.extend_from_slice(pool.as_bytes());
    assert_eq!(u16::from_le_bytes([v3[4], v3[5]]), 3);
    let parsed = UiScreen::parse(&v3).unwrap();
    let depths: Vec<i8> = (0..parsed.len()).map(|i| parsed.node(i).depth).collect();
    assert_eq!(depths, [0, 8, 0, -4, 0]);
    // The same screen as version 2 wrote it.
    let mut v2 = v3[..tuil::HEADER].to_vec();
    v2[4..6].copy_from_slice(&2u16.to_le_bytes());
    v2[14..16].copy_from_slice(&(tuil::RECORD_V2 as u16).to_le_bytes());
    let pool_at = (tuil::HEADER + s.len() * tuil::RECORD_V2) as u32;
    v2[32..36].copy_from_slice(&pool_at.to_le_bytes());
    for r in &records {
        v2.extend_from_slice(&r[..tuil::RECORD_V2]);
    }
    v2.extend_from_slice(pool.as_bytes());
    let old = UiScreen::parse(&v2).unwrap();
    assert!((0..old.len()).all(|i| old.node(i).depth == 0));
    assert_eq!(old.node(3).offset, (20, 0));
    // A version 3 header with version 2's record size is short.
    let mut short = v3.clone();
    short[14..16].copy_from_slice(&(tuil::RECORD_V2 as u16).to_le_bytes());
    assert!(UiScreen::parse(&short).is_err());
}

/// A minimal stereo backend: records the top screen while the slider is
/// up, then replays it per eye with the 2D moved by the layer depth —
/// what the 3DS and the PC's 3DS profile do. Its fills land per eye.
struct Backend {
    frame: StereoFrame,
    recording: bool,
    eye: Eye,
    depth: LayerDepth,
    cmds: Vec<StereoCmd>,
    text: Vec<u8>,
    drawn: Vec<(Eye, f32)>,
    texts: Vec<(Eye, String)>,
}

impl Backend {
    fn new() -> Backend {
        Backend {
            frame: StereoFrame::mono(StereoMono::Off),
            recording: false,
            eye: Eye::Mono,
            depth: LayerDepth::SCREEN,
            cmds: Vec::new(),
            text: Vec::new(),
            drawn: Vec::new(),
            texts: Vec::new(),
        }
    }
}

impl Renderer for Backend {
    fn caps(&self) -> &Caps {
        &Caps::N3DS
    }
    fn begin_frame(&mut self, _: Color) {
        self.frame = StereoFrame::mono(StereoMono::Off);
        self.cmds.clear();
        self.text.clear();
        self.depth = LayerDepth::SCREEN;
    }
    fn set_stereo(&mut self, s: Option<&StereoSettings>) {
        self.frame = StereoFrame::resolve(s, self.caps().stereo, 0.0, true);
        self.recording = self.frame.is_stereo();
    }
    fn stereo_eye(&self) -> Eye {
        self.eye
    }
    fn set_layer_depth(&mut self, d: LayerDepth) {
        if self.recording {
            self.cmds.push(StereoCmd::Depth(d));
        } else {
            self.depth = d;
        }
    }
    fn fill_rect(&mut self, r: Rect, color: Color) {
        if self.recording {
            self.cmds.push(StereoCmd::Rect { rect: r, color });
            return;
        }
        let x = stereo::shift_2d(r.pos, self.depth, self.eye, self.frame.strength).x;
        self.drawn.push((self.eye, x));
    }
    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        if self.recording {
            let start = self.text.len() as u32;
            self.text.extend_from_slice(text.as_bytes());
            self.cmds.push(StereoCmd::Text {
                font,
                pos,
                text: stereo::Span {
                    start,
                    len: text.len() as u32,
                },
                params: *params,
            });
            return;
        }
        self.texts.push((self.eye, text.to_string()));
    }
    fn end_frame(&mut self) {
        if !self.recording {
            return;
        }
        self.recording = false;
        let cmds = std::mem::take(&mut self.cmds);
        let text = std::mem::take(&mut self.text);
        for eye in [Eye::Left, Eye::Right] {
            self.eye = eye;
            self.depth = LayerDepth::SCREEN;
            let data = StereoData {
                text: &text,
                ..StereoData::default()
            };
            stereo::replay(self, &cmds, &data);
        }
        self.eye = Eye::Mono;
    }
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn draw_model(&mut self, _: ModelId, _: &Transform3, _: Material, _: &ModelParams) {}
}

#[test]
fn a_frame_is_drawn_once_and_replayed_per_eye() {
    let mut b = Backend::new();
    let settings = StereoSettings::new(0.1, 4.0);
    b.begin_frame(Color::BLACK);
    b.set_stereo(Some(&settings.with_slider(1.0)));
    b.fill_rect(Rect::new(100.0, 0.0, 4.0, 4.0), Color::WHITE);
    b.set_layer_depth(LayerDepth(10));
    b.fill_rect(Rect::new(200.0, 0.0, 4.0, 4.0), Color::WHITE);
    b.draw_text(FontId(1), Vec2::ZERO, "HUD", &TextParams::DEFAULT);
    assert_eq!(b.stereo_eye(), Eye::Mono, "the game draws once, as mono");
    assert!(b.drawn.is_empty(), "nothing drawn before the replay");
    b.end_frame();
    assert_eq!(
        b.drawn,
        [
            (Eye::Left, 100.0),
            (Eye::Left, 205.0),
            (Eye::Right, 100.0),
            (Eye::Right, 195.0)
        ]
    );
    assert_eq!(
        b.texts,
        [
            (Eye::Left, "HUD".to_string()),
            (Eye::Right, "HUD".to_string())
        ]
    );
    // The slider at 0: one picture, drawn as the game drew it.
    b.drawn.clear();
    b.begin_frame(Color::BLACK);
    b.set_stereo(Some(&settings.with_slider(0.0)));
    b.set_layer_depth(LayerDepth(10));
    b.fill_rect(Rect::new(200.0, 0.0, 4.0, 4.0), Color::WHITE);
    b.end_frame();
    assert_eq!(b.drawn, [(Eye::Mono, 200.0)]);
}
