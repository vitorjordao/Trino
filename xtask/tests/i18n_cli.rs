//! `cargo xtask i18n check|pseudo|export|import|stats|fonts|list` through
//! the real binary (issue #59): the loop an agent runs after adding a
//! language by hand, on a fixture with one problem of each kind and on the
//! tables the repo ships.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn xtask(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", text(&o.stdout), text(&o.stderr)))
}

/// `(severity, kind, lang, key, message)` of every issue.
fn issues(r: &serde_json::Value) -> Vec<(String, String, String, String, String)> {
    r["issues"]
        .as_array()
        .unwrap()
        .iter()
        .map(|i| {
            let s = |k: &str| i[k].as_str().unwrap_or("").to_string();
            (s("severity"), s("kind"), s("lang"), s("key"), s("message"))
        })
        .collect()
}

#[test]
fn i18n_check_is_clean_on_the_tables_the_repo_ships() {
    for dir in [".", "examples/solid-eleven", "templates/standalone"] {
        let o = xtask(&["i18n", "check", "--dir", dir, "--json"]);
        let r = json(&o);
        assert!(o.status.success(), "{dir}: {:#?}", issues(&r));
        assert_eq!(r["errors"], 0, "{dir}");
    }
    let r = json(&xtask(&["i18n", "check", "--json"]));
    assert_eq!(
        r["languages"],
        serde_json::json!(["en-US", "es-ES", "pt-BR"])
    );
    assert_eq!(r["default"], "en-US");
    assert_eq!(r["targets"].as_array().unwrap().len(), 4);
}

const FIXTURE_MANIFEST: &str = r#"version = 1

[i18n]
default = "en-US"
check_fonts = ["fonts/small", "fonts/huge"]

[fonts.small]
file = "fonts/roboto.ttf"
size = 11
chars = " ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789,.-?"

[fonts.huge]
file = "fonts/roboto.ttf"
size = 44
chars = "auto"
"#;

const SCREEN: &str = r#"(
    version: 1,
    theme: (font: "fonts/small"),
    root: (kind: Stack(), anchor: Center, children: [
        (kind: Button, id: "go", key: "menu.play", text: "Play", size: (Px(48), Px(16)),
         on_click: "go"),
    ]),
)"#;

/// A project with one problem of each kind the check reports.
fn fixture(dir: &Path) {
    for d in ["assets/i18n", "assets/shared/fonts", "ui", "src"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::copy(
        root().join("assets/shared/fonts/RobotoCondensed-Bold.ttf"),
        dir.join("assets/shared/fonts/roboto.ttf"),
    )
    .unwrap();
    std::fs::write(dir.join("assets/manifest.toml"), FIXTURE_MANIFEST).unwrap();
    std::fs::write(dir.join("ui/menu.ui.ron"), SCREEN).unwrap();
    std::fs::write(
        dir.join("src/lib.rs"),
        "const K: &str = \"guests.count\";\nconst W: &str = \"welcome\";\n",
    )
    .unwrap();
    let tables = [
        (
            "en-US",
            r#"Strings(lang: "en-US", entries: {
                "menu.play": "Play now",
                "welcome": "Hi {name}",
                "guests.count": Plural(one: "{n} guest", other: "{n} guests"),
                "never.used": "Old",
            })"#,
        ),
        (
            // Overflow (a long label in a 48 px button), a glyph the small
            // font lacks (ç), a renamed placeholder, an extra key.
            "pt-BR",
            r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {
                "menu.play": "Começar a partida",
                "welcome": "Oi {nome}",
                "guests.count": Plural(one: "{n} convidado", other: "{n} convidados"),
                "never.used": "Velho",
                "stray": "Sobra",
            })"#,
        ),
        (
            // No fallback: a missing key shows nothing; a Slavic plural
            // without few/many.
            "ru",
            r#"Strings(lang: "ru", entries: {
                "menu.play": "Igrat",
                "welcome": "Privet {name}",
                "guests.count": Plural(one: "{n} gost", other: "{n} gostej"),
            })"#,
        ),
    ];
    for (lang, t) in tables {
        std::fs::write(dir.join(format!("assets/i18n/{lang}.ron")), t).unwrap();
    }
}

#[test]
fn i18n_check_reports_each_kind_of_problem() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let dir = tmp.path().to_str().unwrap();
    let o = xtask(&["i18n", "check", "--dir", dir, "--platform", "n64", "--json"]);
    assert!(!o.status.success(), "errors exit non-zero");
    let r = json(&o);
    let all = issues(&r);
    let has = |sev: &str, kind: &str, lang: &str, key: &str| {
        all.iter()
            .any(|i| i.0 == sev && i.1 == kind && i.2 == lang && (key.is_empty() || i.3 == key))
    };
    assert!(has("error", "missing_key", "ru", "never.used"), "{all:#?}");
    assert!(has("error", "placeholder", "pt-BR", "welcome"), "{all:#?}");
    assert!(
        has("warning", "plural_form", "ru", "guests.count"),
        "{all:#?}"
    );
    assert!(has("warning", "extra_key", "pt-BR", "stray"), "{all:#?}");
    assert!(has("info", "unused_key", "en-US", "never.used"), "{all:#?}");
    assert!(
        !has("info", "unused_key", "en-US", "guests.count"),
        "a key in the code's literals is used"
    );
    assert!(has("error", "missing_glyph", "pt-BR", ""), "{all:#?}");
    assert!(
        all.iter()
            .any(|i| i.1 == "missing_glyph" && i.4.contains('ç')),
        "{all:#?}"
    );
    assert!(has("error", "overflow", "pt-BR", "menu.play"), "{all:#?}");
    assert!(!has("error", "overflow", "en-US", "menu.play"), "{all:#?}");
    assert!(has("warning", "font_budget", "*", ""), "{all:#?}");
    // Fixed: the check passes, and only warnings remain unless --strict.
    let lib = tmp.path().join("assets/i18n");
    std::fs::write(
        lib.join("ru.ron"),
        r#"Strings(lang: "ru", fallback: "en-US", entries: {"menu.play": "Igrat"})"#,
    )
    .unwrap();
    std::fs::write(
        lib.join("pt-BR.ron"),
        r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"menu.play": "Ir"})"#,
    )
    .unwrap();
    let o = xtask(&["i18n", "check", "--dir", dir, "--platform", "n64", "--json"]);
    assert!(o.status.success(), "{:#?}", issues(&json(&o)));
    let o = xtask(&[
        "i18n",
        "check",
        "--dir",
        dir,
        "--platform",
        "n64",
        "--strict",
    ]);
    assert!(!o.status.success(), "--strict fails on the warnings");
}

#[test]
fn pseudo_and_csv_round_trip_through_the_files() {
    let tmp = tempfile::tempdir().unwrap();
    fixture(tmp.path());
    let dir = tmp.path().to_str().unwrap();
    let o = xtask(&["i18n", "pseudo", "--dir", dir, "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let pseudo = tmp.path().join("assets/i18n/qps-ploc.ron");
    let t = std::fs::read_to_string(&pseudo).unwrap();
    assert!(t.contains("[Plàý ñöw"), "{t}");
    let r = json(&xtask(&["i18n", "list", "--dir", dir, "--json"]));
    let row = r
        .as_array()
        .unwrap()
        .iter()
        .find(|r| r["lang"] == "qps-ploc")
        .unwrap();
    assert_eq!(
        (row["pseudo"].clone(), row["shipped"].clone()),
        (true.into(), false.into())
    );
    // The pseudo text overflows the tight button: a warning, not an error.
    let r = json(&xtask(&[
        "i18n",
        "check",
        "--dir",
        dir,
        "--platform",
        "pc",
        "--json",
    ]));
    assert!(
        issues(&r)
            .iter()
            .any(|i| i.0 == "warning" && i.1 == "overflow" && i.2 == "qps-ploc"),
        "{:#?}",
        issues(&r)
    );
    std::fs::remove_file(&pseudo).unwrap();
    // CSV: export, import unchanged, then a translator's edit.
    let csv = tmp.path().join("strings.csv");
    let o = xtask(&[
        "i18n",
        "export",
        "--csv",
        csv.to_str().unwrap(),
        "--dir",
        dir,
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&xtask(&[
        "i18n",
        "import",
        "--csv",
        csv.to_str().unwrap(),
        "--dir",
        dir,
        "--json",
    ]));
    assert_eq!(
        (r["added"].clone(), r["changed"].clone()),
        (0.into(), 0.into())
    );
    let edited = std::fs::read_to_string(&csv)
        .unwrap()
        .replace("Começar a partida", "Jogar");
    std::fs::write(&csv, edited).unwrap();
    let r = json(&xtask(&[
        "i18n",
        "import",
        "--csv",
        csv.to_str().unwrap(),
        "--dir",
        dir,
        "--json",
    ]));
    assert_eq!(r["changed"], 1);
    let pt = std::fs::read_to_string(tmp.path().join("assets/i18n/pt-BR.ron")).unwrap();
    assert!(pt.contains("\"menu.play\": \"Jogar\""), "{pt}");
}

#[test]
fn stats_and_fonts_report_per_language() {
    let r = json(&xtask(&["i18n", "stats", "--json"]));
    let en = r
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["lang"] == "en-US")
        .unwrap();
    // The self-test's four, castle64's three captions (#60) and its
    // achievements' and presence's sixteen (#61).
    assert_eq!(en["keys"], 23);
    let r = json(&xtask(&["i18n", "fonts", "--platform", "n64", "--json"]));
    let atlases: Vec<(&str, &str)> = r
        .as_array()
        .unwrap()
        .iter()
        .filter(|c| c["font"].as_str().unwrap().starts_with("fonts/i18n"))
        .map(|c| (c["font"].as_str().unwrap(), c["lang"].as_str().unwrap()))
        .collect();
    assert_eq!(
        atlases,
        [
            ("fonts/i18n", "en-US"),
            ("fonts/i18n@es-ES", "es-ES"),
            ("fonts/i18n@pt-BR", "pt-BR")
        ]
    );
    for c in r.as_array().unwrap() {
        if c["font"].as_str().unwrap().starts_with("fonts/i18n") {
            assert_eq!(c["memory"], "TMEM");
            assert_eq!(c["fits"], true, "{c}");
        }
    }
}

#[test]
fn i18n_export_writes_static_code_and_the_committed_export_is_fresh() {
    let tmp = tempfile::tempdir().unwrap();
    let out = tmp.path().join("strings_gen.rs");
    let o = xtask(&[
        "i18n",
        "export",
        "--rust",
        out.to_str().unwrap(),
        "--dir",
        "examples/solid-eleven",
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    assert_eq!(r["languages"], serde_json::json!(["en-US", "pt-BR"]));
    let fresh = std::fs::read_to_string(&out).unwrap();
    assert!(fresh.contains("pub const PT_BR"), "{fresh}");
    let committed =
        std::fs::read_to_string(root().join("examples/solid-eleven/src/ui/strings_gen.rs"))
            .unwrap();
    assert!(
        committed == fresh,
        "examples/solid-eleven/src/ui/strings_gen.rs is stale: run `cargo xtask i18n export \
         --rust examples/solid-eleven/src/ui/strings_gen.rs --dir examples/solid-eleven`"
    );
    // The static tables are the bake's bytes.
    for (i, lang) in ["en-US", "pt-BR"].iter().enumerate() {
        let t = trino_scene::i18n::StringsDef::load(
            &root().join(format!("examples/solid-eleven/assets/i18n/{lang}.ron")),
        )
        .unwrap();
        let baked = t.to_tstr().unwrap();
        let parsed = trino_core::i18n::StringTable::parse(&baked).unwrap();
        let (h, f, rec, pool) = solid_eleven::ui::i18n::TABLES[i].parts();
        assert_eq!(parsed.parts(), (h, f, rec, pool), "{lang}");
    }
}
