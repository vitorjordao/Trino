//! The UI system on the PC renderer (issue #58): the safe area each
//! profile reports, and strict mode failing a screen past the profile's
//! UI budget (`Caps::max_ui_nodes`, `Caps::max_ui_draws`) the way it fails
//! a texture past the N64's TMEM.

use trino_core::ui::{Anchor, NONE, NodeKind, Nodes, Scaling, Size, Theme, UiNode, UiScreen};
use trino_core::{Caps, Color, Insets, Renderer, Screen, Ui, UiEnv};
use trino_platform_pc::{PcRenderer, SimProfile};

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => Some(r),
        Err(e) => {
            assert!(
                std::env::var("TRINO_REQUIRE_GPU").is_err(),
                "TRINO_REQUIRE_GPU set but renderer init failed: {e}"
            );
            eprintln!("skipping: {e}");
            None
        }
    }
}

/// A root holding `n` small solid rects: `n + 1` nodes, `n` fills.
fn rects(n: usize) -> Vec<UiNode<'static>> {
    let mut nodes = vec![UiNode {
        end: n as u16 + 1,
        size: (Size::Fill, Size::Fill),
        ..UiNode::DEFAULT
    }];
    for i in 0..n {
        nodes.push(UiNode {
            kind: NodeKind::Rect {
                border: 0,
                border_color: Color::BLACK,
            },
            parent: 0,
            end: i as u16 + 2,
            anchor: Anchor::TopLeft,
            offset: ((i % 16) as i16 * 4, (i / 16) as i16 * 4),
            size: (Size::Px(2), Size::Px(2)),
            color: Some(Color::WHITE),
            ..UiNode::DEFAULT
        });
    }
    nodes
}

fn screen<'a>(nodes: &'a [UiNode<'a>]) -> UiScreen<'a> {
    UiScreen {
        name: "rects",
        reference: (320, 240),
        scaling: Scaling::Integer,
        wrap: false,
        focus: NONE,
        on_back: "",
        theme: Theme::DEFAULT,
        nodes: Nodes::Slice(nodes),
    }
}

fn draw(r: &mut PcRenderer, nodes: &[UiNode<'_>]) {
    let s = screen(nodes);
    let mut ui: Box<Ui<512>> = Box::new(Ui::new());
    r.begin_frame(Color::BLACK);
    let env = UiEnv::of(r);
    ui.draw(r, &s, &env);
    r.end_frame();
}

#[test]
fn strict_mode_fails_a_ui_screen_over_the_n64_node_budget() {
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    r.set_strict(true);
    // 64 nodes: exactly the N64's budget.
    let fits = rects(Caps::N64.max_ui_nodes as usize - 1);
    draw(&mut r, &fits);
    let stats = r.last_ui_stats().expect("the UI reported its counts");
    assert_eq!(stats.nodes, Caps::N64.max_ui_nodes);
    assert_eq!(stats.draws, Caps::N64.max_ui_nodes - 1, "one fill per rect");
    // One more fails, naming the cap and the check that would have caught it.
    let over = rects(Caps::N64.max_ui_nodes as usize);
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw(&mut r, &over)))
        .expect_err("strict mode must reject 65 nodes on the N64");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(
        msg.contains("Caps::max_ui_nodes") && msg.contains("ui check"),
        "{msg}"
    );
    // The PC's budget is far bigger, and without strict mode nothing fails.
    let Some(mut pc) = headless(SimProfile::Native) else {
        return;
    };
    pc.set_strict(true);
    draw(&mut pc, &over);
    assert_eq!(pc.last_ui_stats().unwrap().nodes, 65);
    r.set_strict(false);
    draw(&mut r, &over);
}

#[test]
fn strict_mode_fails_a_ui_screen_over_the_n64_draw_budget() {
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    r.set_strict(true);
    // 40 rects with a long text each: few nodes, a glyph draw per letter.
    let text = "ABCDEFGHIJ";
    let mut nodes = rects(40);
    for n in nodes.iter_mut().skip(1) {
        n.kind = NodeKind::Text;
        n.text = text;
        n.size = (Size::Px(64), Size::Px(10));
    }
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| draw(&mut r, &nodes)))
        .expect_err("400 glyphs are over the N64's 384 draws");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("Caps::max_ui_draws"), "{msg}");
}

#[test]
fn safe_areas_follow_the_profile_and_an_override() {
    let Some(mut n64) = headless(SimProfile::N64) else {
        return;
    };
    // The TV's overscan on the N64, nothing on the touch screen.
    assert_eq!(n64.safe_area(Screen::Top), Insets::symmetric(16, 12));
    assert_eq!(n64.safe_area(Screen::Bottom), Insets::ZERO);
    let custom = Insets {
        left: 1,
        top: 2,
        right: 3,
        bottom: 4,
    };
    n64.set_safe_area(Some(custom));
    assert_eq!(n64.safe_area(Screen::Top), custom);
    n64.set_safe_area(None);
    assert_eq!(n64.safe_area(Screen::Top), Insets::symmetric(16, 12));
    // Window insets need a window: a headless renderer keeps its own.
    n64.set_window_safe_area([100.0, 0.0, 0.0, 0.0]);
    assert_eq!(n64.safe_area(Screen::Top), Insets::symmetric(16, 12));
    for profile in [SimProfile::N3ds, SimProfile::Native, SimProfile::Web] {
        let Some(r) = headless(profile) else {
            return;
        };
        assert_eq!(r.safe_area(Screen::Top), Insets::ZERO, "{profile:?}");
    }
}
