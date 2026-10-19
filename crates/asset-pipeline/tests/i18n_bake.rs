//! Localization in the bake (issue #59): `chars = "auto"` subsets a font to
//! exactly the glyphs the shipped languages use, `per_language` bakes one
//! atlas per language (in its own asset group), each language's `TSTR`
//! names the atlas it draws with, pseudo-locales stay off the consoles, and
//! the TMEM report sees every atlas.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use trino_asset_pipeline::{Manifest, Platform, bake_all, i18n};
use trino_core::i18n::{LangId, Localizer, StrKey, StringTable};
use trino_core::{FontId, GroupId, asset_id};

fn roboto() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../examples/solid-eleven/assets/shared/fonts/RobotoCondensed-Bold.ttf")
}

const EN: &str = r#"Strings(lang: "en-US", name: "English", entries: {
    "menu.play": "PLAY",
    "menu.coins": Plural(one: "{n} COIN", other: "{n} COINS"),
})"#;
const PT: &str = r#"Strings(lang: "pt-BR", fallback: "en-US", name: "PORTUGUÊS", entries: {
    "menu.play": "JOGAR",
    "menu.coins": Plural(one: "{n} MOEDA", other: "{n} MOEDAS"),
})"#;
const ES: &str = r#"Strings(lang: "es-ES", fallback: "en-US", name: "ESPAÑOL", entries: {
    "menu.play": "JUGAR",
})"#;
const PSEUDO: &str = r#"Strings(lang: "qps-ploc", fallback: "en-US", entries: {
    "menu.play": "[ƤĹÀŶ ~~]",
})"#;
const SCREEN: &str = r#"(
    version: 1,
    root: (kind: Stack(), anchor: Center, children: [
        (kind: Text, text: "V1"),
        (kind: Button, id: "play", key: "menu.play", text: "PLAY", on_click: "play"),
    ]),
)"#;

fn project(root: &Path, manifest: &str) {
    for d in ["assets/i18n", "assets/shared/fonts", "ui"] {
        std::fs::create_dir_all(root.join(d)).unwrap();
    }
    std::fs::copy(roboto(), root.join("assets/shared/fonts/roboto.ttf")).unwrap();
    std::fs::write(root.join("assets/shared/fonts/grid.png"), [0u8; 0]).unwrap();
    std::fs::write(root.join("assets/manifest.toml"), manifest).unwrap();
    for (f, t) in [
        ("en-US", EN),
        ("pt-BR", PT),
        ("es-ES", ES),
        ("qps-ploc", PSEUDO),
    ] {
        std::fs::write(root.join(format!("assets/i18n/{f}.ron")), t).unwrap();
    }
    std::fs::write(root.join("ui/menu.ui.ron"), SCREEN).unwrap();
}

const MANIFEST: &str = r##"version = 1

[i18n]
default = "en-US"
extra_chars = "#"
groups = true

[i18n.overrides.es-ES]
"fonts/title" = { pc = "fonts/ui", web = "fonts/ui" }

[fonts.ui]
file = "fonts/roboto.ttf"
size = 11
chars = "auto"
extra_chars = "*"
per_language = true

[fonts.all]
file = "fonts/roboto.ttf"
size = 11
chars = "auto"

[fonts.title]
file = "fonts/roboto.ttf"
size = 11
chars = "ABC"
"##;

fn chars(m: &Manifest, name: &str) -> BTreeSet<char> {
    m.fonts[name].chars.as_deref().unwrap().chars().collect()
}

#[test]
fn auto_fonts_bake_exactly_the_glyphs_the_languages_use() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), MANIFEST);
    let m = Manifest::load(&dir.path().join("assets/manifest.toml")).unwrap();
    // The default language's atlas: its glyphs, the screen's literal text,
    // the extras; no Ê (pt-BR), no Ñ (es-ES), no pseudo glyph.
    let en = chars(&m, "ui");
    for c in "PLAYCOINS V1#*0123456789-.,$/English".chars() {
        assert!(en.contains(&c), "en atlas lacks {c:?}");
    }
    for c in "ÊÑƤJ".chars() {
        assert!(!en.contains(&c), "en atlas has {c:?}");
    }
    let pt = chars(&m, "ui@pt-BR");
    assert!(pt.contains(&'Ê') && pt.contains(&'J') && !pt.contains(&'Ñ'));
    assert!(
        pt.contains(&'V') && pt.contains(&'*'),
        "shared text and extras"
    );
    let es = chars(&m, "ui@es-ES");
    assert!(es.contains(&'Ñ') && !es.contains(&'Ê'));
    // `all` (no per_language) is the union, still without the pseudo-locale.
    let all = chars(&m, "all");
    assert!(all.is_superset(&pt) || all.len() >= pt.len());
    for c in "ÊÑJ".chars() {
        assert!(all.contains(&c));
    }
    assert!(!all.contains(&'Ƥ'), "pseudo-locales never add glyphs");
    assert!(
        !all.contains(&'Q') && !all.contains(&'Z'),
        "an unused glyph is not baked"
    );
    assert!(
        all.len() > en.len() && all.len() > es.len(),
        "per-language atlases are smaller"
    );
    // An explicit font is left alone.
    assert_eq!(m.fonts["title"].chars.as_deref(), Some("ABC"));
    // The per-language atlases sit in their language's group.
    assert_eq!(m.fonts["ui@pt-BR"].groups, ["i18n-pt-br"]);
    assert!(m.groups.contains_key("i18n-pt-br") && m.groups.contains_key("i18n-es-es"));
    assert!(
        m.fonts["ui"].groups.is_empty(),
        "the default's atlas stays in boot"
    );
    let r = &m.i18n.as_ref().unwrap().resolved;
    assert_eq!(r.languages, ["en-US", "es-ES", "pt-BR"]);
    assert_eq!(r.default.as_deref(), Some("en-US"));
}

#[test]
fn each_languages_table_names_its_atlas_and_group() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), MANIFEST);
    for platform in [Platform::Pc, Platform::Web, Platform::N64, Platform::N3ds] {
        let out = dir.path().join(format!("out-{platform:?}"));
        let report = bake_all(&dir.path().join("assets"), platform, &out)
            .unwrap_or_else(|e| panic!("{platform:?}: {e}"));
        let blob = |logical: &str| -> Option<&'static [u8]> {
            let e = report.entries.iter().find(|e| e.logical == logical)?;
            let path = if e.file.is_empty() {
                return None;
            } else {
                out.join(&e.file)
            };
            std::fs::read(path)
                .ok()
                .map(|b| &*Box::leak(b.into_boxed_slice()))
        };
        let list = std::str::from_utf8(blob("i18n/languages").unwrap()).unwrap();
        let pseudo = matches!(platform, Platform::Pc | Platform::Web);
        let want = if pseudo {
            "en-US\nes-ES\npt-BR\nqps-ploc\n"
        } else {
            "en-US\nes-ES\npt-BR\n"
        };
        assert_eq!(list, want, "{platform:?}");
        assert_eq!(blob("i18n/qps-ploc").is_some(), pseudo, "{platform:?}");
        let pt = StringTable::parse(blob("i18n/pt-BR").unwrap()).unwrap();
        let ui = FontId::from_path("fonts/ui");
        assert_eq!(pt.font(ui), FontId::from_path("fonts/ui@pt-BR"));
        assert_eq!(pt.group(), Some(GroupId::from_name("i18n-pt-br")));
        assert_eq!(pt.name(), "PORTUGUÊS");
        let en = StringTable::parse(blob("i18n/en-US").unwrap()).unwrap();
        assert_eq!(en.font(ui), ui, "the default draws the base atlas");
        assert_eq!(en.group(), None);
        // The title font has an override for es-ES on the PC and the Web only.
        let es = StringTable::parse(blob("i18n/es-ES").unwrap()).unwrap();
        let title = FontId::from_path("fonts/title");
        let want = if pseudo {
            FontId::from_path("fonts/ui")
        } else {
            title
        };
        assert_eq!(es.font(title), want, "{platform:?}");
        // The runtime reads the same tables.
        let mut loc: Localizer<'static, 4> = Localizer::new();
        for t in list.lines() {
            loc.add(StringTable::parse(blob(&format!("i18n/{t}")).unwrap()).unwrap())
                .unwrap();
        }
        assert!(loc.set_language(LangId::new("pt-BR")));
        assert_eq!(loc.plural(StrKey::new("menu.coins"), 1), "{n} MOEDA");
        assert_eq!(loc.font(ui), FontId::from_path("fonts/ui@pt-BR"));
        // The pt-BR atlas is a font of its own on every target.
        assert!(
            report
                .entries
                .iter()
                .any(|e| e.id == asset_id("fonts/ui@pt-BR")),
            "{platform:?}"
        );
    }
}

#[test]
fn the_font_report_sees_every_atlas_and_its_tmem() {
    let dir = tempfile::tempdir().unwrap();
    project(dir.path(), MANIFEST);
    let assets = dir.path().join("assets");
    let m = Manifest::load(&assets.join("manifest.toml")).unwrap();
    let costs = i18n::font_costs(&assets, &m, &[Platform::N64, Platform::N3ds]).unwrap();
    let ui_pt = costs
        .iter()
        .find(|c| c.font == "fonts/ui@pt-BR" && c.platform == "n64")
        .unwrap();
    assert_eq!(ui_pt.lang, "pt-BR");
    assert_eq!(ui_pt.memory, "TMEM");
    assert_eq!(ui_pt.budget, Some(4096));
    assert_eq!(ui_pt.group, "i18n-pt-br");
    let base = costs
        .iter()
        .find(|c| c.font == "fonts/ui" && c.platform == "n64")
        .unwrap();
    assert_eq!(base.lang, "en-US");
    let all = costs
        .iter()
        .find(|c| c.font == "fonts/all" && c.platform == "n64")
        .unwrap();
    assert!(all.glyphs > ui_pt.glyphs && all.bytes >= ui_pt.bytes);
    assert_eq!(
        i18n::font_for(&m, "fonts/ui", "pt-BR", Platform::N64),
        "fonts/ui@pt-BR"
    );
    let checked = i18n::checked_fonts(&m, dir.path());
    assert!(checked.contains(&"fonts/ui".to_string()));
    assert!(checked.contains(&"fonts/all".to_string()));
}

#[test]
fn bad_i18n_declarations_fail_with_their_reason() {
    let cases = [
        (
            "[fonts.g]\nfile = \"fonts/grid.png\"\ncell = [6, 10]\nchars = \"auto\"\n",
            "needs a TTF/OTF",
        ),
        (
            "[fonts.u]\nfile = \"fonts/roboto.ttf\"\nsize = 9\nper_language = true\n",
            "per_language needs",
        ),
        ("[i18n]\nlanguages = [\"ja-JP\"]\n", "ja-JP"),
        ("[i18n]\ndefault = \"de\"\n", "default"),
        (
            "[i18n.overrides.pt-BR]\n\"fonts/x\" = \"fonts/y\"\n",
            "not a font",
        ),
    ];
    for (extra, want) in cases {
        let dir = tempfile::tempdir().unwrap();
        project(dir.path(), &format!("version = 1\n{extra}"));
        let e = Manifest::load(&dir.path().join("assets/manifest.toml")).unwrap_err();
        assert!(e.contains(want), "{extra}: {e}");
    }
}
