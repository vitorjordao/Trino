//! `ui/*.ui.ron` (issue #58): the format, validation, flattening, the TUIL
//! bake, the Rust export, the audit `ui check` runs, string tables.

use trino_core::ui::{NONE, NodeKind, UiScreen, UiStrings};
use trino_core::{Caps, Color, Family, FontId, Screen, SpriteId};
use trino_scene::ui::{
    Anchor, Flat, Fonts, Kind, Langs, Level, NodeDef, Sample, ScreenDef, Size, StringsDef, Target,
    audit, screen_name, to_rust, to_tuil, validate,
};

const PAUSE: &str = r#"(
    version: 1,
    name: "pause",
    on_back: "resume",
    focus: "resume",
    root: (
        children: [
            (kind: Rect(), anchor: Fill, bleed: true, color: Some((0, 0, 0, 160))),
            (
                kind: Stack(gap: 4, align: Center),
                anchor: Center,
                padding: (8, 8, 8, 8),
                children: [
                    (kind: Text, key: "pause.title", text: "PAUSED"),
                    (kind: Button, id: "resume", text: "RESUME", on_click: "resume", size: (Px(96), Px(16))),
                    (kind: Button, id: "quit", text: "QUIT", on_click: "quit", size: (Px(96), Px(16)), nav: (down: "resume")),
                ],
            ),
        ],
    ),
)"#;

/// The engine font, measured as its 6 × 10 cell.
fn engine_font() -> Fonts {
    let mut fonts = Fonts::default();
    fonts.assume(trino_scene::ui::DEFAULT_FONT);
    fonts
}

fn no_sprites(_: &str) -> Option<(u16, u16)> {
    None
}

#[test]
fn the_format_reads_with_defaults_and_saves_only_what_differs() {
    let def = ScreenDef::from_ron(PAUSE).unwrap();
    assert_eq!(def.name, "pause");
    assert_eq!(def.reference, (320, 240));
    assert_eq!(def.root.children.len(), 2);
    let stack = &def.root.children[1];
    assert_eq!(stack.anchor, Anchor::Center);
    assert_eq!(stack.children[1].size, (Size::Px(96), Size::Px(16)));
    let text = def.to_ron();
    assert!(!text.contains("visible_on"), "{text}");
    assert!(!text.contains("offset"), "{text}");
    assert_eq!(ScreenDef::from_ron(&text).unwrap(), def);
    // A newer file is refused, and a bare struct kind gets a hint.
    let newer = PAUSE.replace("version: 1", "version: 2");
    assert!(ScreenDef::from_ron(&newer).unwrap_err().contains("newer"));
    let bare = "(version: 1, root: (kind: Stack))";
    assert!(ScreenDef::from_ron(bare).unwrap_err().contains("`Stack()`"));
    assert_eq!(
        screen_name(std::path::Path::new("ui/title.ui.ron")),
        "title"
    );
}

#[test]
fn flattening_keeps_depth_first_order_paths_screens_and_nav_ids() {
    let mut def = ScreenDef::from_ron(PAUSE).unwrap();
    def.root.children.push(NodeDef {
        screen: trino_scene::ui::Side::Bottom,
        children: vec![NodeDef {
            id: "map".into(),
            on_click: "map".into(),
            text: "MAP".into(),
            ..NodeDef::new(Kind::Button)
        }],
        ..NodeDef::new(Kind::Overlay)
    });
    let flat = Flat::new(&def, &no_sprites);
    let s = flat.screen();
    s.check_tree().unwrap();
    assert_eq!(s.len(), 8);
    assert_eq!(flat.paths[4], vec![1, 1]);
    assert_eq!(flat.index_of(&[1, 2]), Some(5));
    assert_eq!(s.node(5).nav, [NONE, 4, NONE, NONE]);
    assert_eq!(s.focus, 4);
    assert_eq!(s.node(7).screen, Screen::Bottom, "children follow");
    assert_eq!(s.node(4).action, "resume");
    assert_eq!(s.node(1).color, Some(Color::rgba(0, 0, 0, 160)));
}

#[test]
fn validation_finds_structural_mistakes() {
    let text = r#"(
        version: 1,
        focus: "nobody",
        root: (children: [
            (kind: Text, id: "a", children: [(kind: Text, text: "x")]),
            (kind: Button, id: "a", text: "A", nav: (up: "ghost", down: "title")),
            (kind: List(max: 0), bind: "n"),
            (kind: Grid(cols: 0)),
            (kind: Slider(min: 5, max: 5, step: 0)),
            (kind: Image(sprite: "")),
            (kind: Toggle, text: "T"),
            (kind: Text),
            (kind: Text, id: "title", text: "T"),
        ]),
    )"#;
    let def = ScreenDef::from_ron(text).unwrap();
    let issues = validate(&def);
    let messages: Vec<String> = issues.iter().map(|i| i.to_string()).collect();
    let has = |level: Level, part: &str| {
        issues
            .iter()
            .any(|i| i.level == level && i.message.contains(part))
    };
    assert!(has(Level::Error, "duplicate id \"a\""), "{messages:#?}");
    assert!(has(Level::Error, "focus: no node has id \"nobody\""));
    assert!(has(Level::Error, "nav.up: no node has id \"ghost\""));
    assert!(has(Level::Error, "nav.down: \"title\" is not a control"));
    assert!(has(Level::Error, "a Text cannot hold children"));
    assert!(has(Level::Error, "exactly one child"));
    assert!(has(Level::Error, "max > 0"));
    assert!(has(Level::Error, "cols > 0"));
    assert!(has(Level::Error, "max > min"));
    assert!(has(Level::Error, "step > 0"));
    assert!(has(Level::Error, "needs a sprite"));
    assert!(has(Level::Warning, "Toggle without bind"));
    assert!(has(Level::Warning, "no text, key or bind"));
    let dup = issues
        .iter()
        .find(|i| i.message.contains("duplicate"))
        .unwrap();
    assert_eq!(dup.node, Some(3));
    assert_eq!(dup.label, "Button \"a\"");
    assert!(validate(&ScreenDef::from_ron(PAUSE).unwrap()).is_empty());
}

#[test]
fn the_tuil_bake_reads_back_as_the_same_screen() {
    let mut def = ScreenDef::from_ron(PAUSE).unwrap();
    def.root.children.push(NodeDef {
        kind: Kind::Image {
            sprite: "sprites/frame".into(),
            nine: [4, 4, 4, 4],
            fit: Default::default(),
            src: None,
        },
        ..NodeDef::default()
    });
    let sizes = |p: &str| (p == "sprites/frame").then_some((24, 16));
    let flat = Flat::new(&def, &sizes);
    let blob = to_tuil(&flat).unwrap();
    let back = UiScreen::parse(&blob).unwrap();
    let s = flat.screen();
    assert_eq!(back.len(), s.len());
    for i in 0..s.len() {
        assert_eq!(back.node(i), s.node(i), "node {i}");
    }
    assert_eq!(back.on_back, "resume");
    let NodeKind::Image { sprite, src, .. } = back.node(back.len() - 1).kind else {
        panic!()
    };
    assert_eq!(sprite, SpriteId::from_path("sprites/frame"));
    assert_eq!(src, (24, 16), "the sprite's size, read at bake time");
    // Strings are pooled once: "resume" (id, action, on_back) appears once.
    let pool = std::str::from_utf8(&blob[blob.len() - 64..]).unwrap_or("");
    assert!(pool.matches("resume").count() <= 1, "{pool}");
}

#[test]
fn the_rust_export_names_every_non_default_field() {
    let def = ScreenDef::from_ron(PAUSE).unwrap();
    let flat = Flat::new(&def, &no_sprites);
    let code = to_rust(&flat);
    assert!(code.starts_with("// @generated"));
    assert!(code.contains("pub static NODES: [UiNode<'static>; 6] = ["));
    assert!(code.contains("anchor: Anchor::Center,"));
    assert!(code.contains("NodeKind::Stack { dir: Dir::Vertical, gap: 4, align: Align::Center, justify: Justify::Start }"));
    assert!(code.contains("nav: [NONE, 4, NONE, NONE],"));
    assert!(code.contains("color: Some(Color::rgba(0, 0, 0, 160)),"));
    assert!(code.contains("pub static SCREEN: UiScreen<'static> = UiScreen {"));
    assert!(code.contains("focus: 4,"));
    assert!(code.contains("theme: Theme::DEFAULT,"));
    // Deterministic.
    assert_eq!(code, to_rust(&Flat::new(&def, &no_sprites)));
}

fn langs() -> Langs {
    let mut langs = Langs::default();
    for text in [
        r#"Strings(lang: "en-US", entries: {"pause.title": "PAUSED"})"#,
        r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"pause.title": "JOGO PAUSADO AGORA MESMO"})"#,
        r#"Strings(lang: "es-ES", fallback: "en-US", entries: {})"#,
    ] {
        let t = StringsDef::from_ron(text).unwrap();
        langs.tables.insert(t.lang.clone(), t);
    }
    langs
}

#[test]
fn string_tables_fall_back_and_bake_to_tstr() {
    let langs = langs();
    let pt = langs.strings("pt-BR");
    assert_eq!(pt.get("pause.title"), Some("JOGO PAUSADO AGORA MESMO"));
    let es = langs.strings("es-ES");
    assert_eq!(es.get("pause.title"), Some("PAUSED"));
    assert!(!es.has_own("pause.title"));
    assert_ne!(pt.revision(), es.revision());
    let blob = langs.tables["pt-BR"].to_tstr().unwrap();
    let table = trino_core::ui::StringTable::parse(&blob).unwrap();
    assert_eq!(
        table.lookup("pause.title"),
        Some("JOGO PAUSADO AGORA MESMO")
    );
    assert_eq!(table.fallback(), "en-US");
}

#[test]
fn the_audit_reports_overflow_per_language_reachability_and_budgets() {
    let mut def = ScreenDef::from_ron(PAUSE).unwrap();
    // The title box is fixed: the Portuguese title overflows it.
    def.root.children[1].children[0].size = (Size::Px(96), Size::Px(10));
    let flat = Flat::new(&def, &no_sprites);
    let fonts = engine_font();
    let langs = langs();
    for caps in [Caps::N64, Caps::N3DS, Caps::PC, Caps::WEB] {
        let r = audit(&flat, Target::of(caps), &fonts, &langs);
        assert!(
            r.issues.iter().all(|i| i.level != Level::Error),
            "{:?}",
            r.issues
        );
        assert_eq!(r.stats.nodes, 6);
        assert!(r.stats.draws > 0);
        let en = audit(
            &flat,
            Target {
                lang: Some("en-US"),
                ..Target::of(caps)
            },
            &fonts,
            &langs,
        );
        assert!(en.overflow.is_empty(), "{:?}", en.issues);
        let pt = audit(
            &flat,
            Target {
                lang: Some("pt-BR"),
                ..Target::of(caps)
            },
            &fonts,
            &langs,
        );
        assert_eq!(pt.overflow, vec![3], "{:?}", pt.issues);
        let issue = &pt.issues[0];
        assert_eq!(issue.platform.as_deref(), Some(caps.family.key()));
        assert_eq!(issue.lang.as_deref(), Some("pt-BR"));
        assert!(issue.message.contains("96×10"), "{issue}");
        let es = audit(
            &flat,
            Target {
                lang: Some("es-ES"),
                ..Target::of(caps)
            },
            &fonts,
            &langs,
        );
        assert!(
            es.issues
                .iter()
                .any(|i| i.message.contains("comes from the fallback")),
            "{:?}",
            es.issues
        );
    }
}

#[test]
fn the_audit_fails_unreachable_controls_on_consoles_and_budgets_everywhere() {
    let text = r#"(
        version: 1,
        root: (children: [
            (kind: Button, id: "a", text: "A", nav: (up: "a", down: "a", left: "a", right: "a")),
            (kind: Button, id: "b", text: "B", anchor: BottomRight),
        ]),
    )"#;
    let def = ScreenDef::from_ron(text).unwrap();
    let flat = Flat::new(&def, &no_sprites);
    let fonts = engine_font();
    let langs = Langs::default();
    let n64 = audit(&flat, Target::of(Caps::N64), &fonts, &langs);
    let issue = n64
        .issues
        .iter()
        .find(|i| i.message.contains("cannot reach"))
        .expect("b is unreachable");
    assert_eq!(issue.level, Level::Error);
    assert_eq!(issue.node, Some(2));
    let pc = audit(&flat, Target::of(Caps::PC), &fonts, &langs);
    let issue = pc
        .issues
        .iter()
        .find(|i| i.message.contains("cannot reach"))
        .unwrap();
    assert_eq!(
        issue.level,
        Level::Warning,
        "the mouse reaches it on the PC"
    );
    // A list of 100 items is past the N64's node budget.
    let text = r#"(
        version: 1,
        preview: {"n": Int(100)},
        root: (children: [
            (kind: List(max: 100), bind: "n", children: [(kind: Rect(), size: (Px(2), Px(2)))]),
        ]),
    )"#;
    let def = ScreenDef::from_ron(text).unwrap();
    let flat = Flat::new(&def, &no_sprites);
    let n64 = audit(&flat, Target::of(Caps::N64), &fonts, &langs);
    assert!(
        n64.issues
            .iter()
            .any(|i| i.level == Level::Error && i.message.contains("max_ui_nodes")),
        "{:?}",
        n64.issues
    );
    let pc = audit(&flat, Target::of(Caps::PC), &fonts, &langs);
    assert!(pc.issues.is_empty(), "{:?}", pc.issues);
}

#[test]
fn the_audit_reports_glyphs_a_font_lacks() {
    let def = ScreenDef::from_ron(
        r#"(version: 1, root: (children: [(kind: Text, text: "AB", font: "fonts/tiny")]))"#,
    )
    .unwrap();
    let flat = Flat::new(&def, &no_sprites);
    let mut fonts = engine_font();
    fonts.insert("fonts/tiny", tiny_font());
    let r = audit(&flat, Target::of(Caps::PC), &fonts, &Langs::default());
    assert!(
        r.issues
            .iter()
            .any(|i| i.message == "font fonts/tiny has no glyph for 'B'"),
        "{:?}",
        r.issues
    );
    // An unknown font is an error.
    let def = ScreenDef::from_ron(
        r#"(version: 1, root: (children: [(kind: Text, text: "A", font: "fonts/none")]))"#,
    )
    .unwrap();
    let flat = Flat::new(&def, &no_sprites);
    let r = audit(&flat, Target::of(Caps::PC), &fonts, &Langs::default());
    assert!(
        r.issues
            .iter()
            .any(|i| i.level == Level::Error && i.message.contains("not in the project")),
        "{:?}",
        r.issues
    );
    assert_eq!(
        FontId::from_path("fonts/tiny"),
        FontId::from_path("fonts/tiny")
    );
}

#[test]
fn previews_set_values_and_list_counts() {
    let text = r#"(
        version: 1,
        preview: {"money": Int(1200), "name": Items(["SWORD", "BOW"]), "shown": Bool(true)},
        root: (children: [
            (kind: Stack(), children: [
                (kind: Text, bind: "money", text: "$ {}"),
                (kind: Text, text: "SECRET", visible_if: "shown"),
                (kind: List(), bind: "count", children: [(kind: Text, bind: "name")]),
            ]),
        ]),
    )"#;
    let def = ScreenDef::from_ron(text).unwrap();
    assert_eq!(def.preview["money"], Sample::Int(1200));
    let flat = Flat::new(&def, &no_sprites);
    let r = audit(
        &flat,
        Target::of(Caps::PC),
        &engine_font(),
        &Langs::default(),
    );
    // root, stack, money, secret, list, two items.
    assert_eq!(r.stats.nodes, 7);
    assert_eq!(Family::Pc.key(), "pc");
}

/// A TFNT v1 with one glyph, 'A' (5 × 8, advance 6).
fn tiny_font() -> Vec<u8> {
    use trino_core::text;
    let mut out = Vec::new();
    out.extend_from_slice(text::TFNT_MAGIC);
    out.extend_from_slice(&text::TFNT_VERSION.to_le_bytes());
    out.extend_from_slice(&8u16.to_le_bytes()); // line height
    out.extend_from_slice(&7u16.to_le_bytes()); // ascent
    out.extend_from_slice(&1u16.to_le_bytes()); // glyphs
    out.extend_from_slice(&('A' as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // x
    out.extend_from_slice(&0u16.to_le_bytes()); // y
    out.extend_from_slice(&[5, 8, 0, 6]); // w, h, x offset, advance
    out
}
