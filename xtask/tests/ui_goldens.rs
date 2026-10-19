//! The starter UI screens (`templates/standalone/ui/`, issue #58) rendered
//! by the PC renderer on the N64 and 3DS profiles and the native one at
//! 1080p, compared with `tests/golden/pc/ui_<screen>_<platform>.png` — the
//! layout, the theme, the fonts and the 3DS's touch screen, pixel for
//! pixel. GPU-gated like the other goldens (`TRINO_REQUIRE_GPU=1` turns a
//! missing adapter into a failure); `TRINO_BLESS=1` (`cargo xtask test
//! --bless`) rewrites them.

#[allow(dead_code)]
#[path = "../src/ui_render.rs"]
mod ui_render;

use std::path::{Path, PathBuf};

use trino_core::Family;
use trino_scene::ui::{Langs, ScreenDef, StringsDef};
use ui_render::ShotOptions;

/// One RGBA5551 step on the N64 profile, two elsewhere (blend rounding).
fn tolerance(family: Family) -> u8 {
    if family == Family::N64 { 9 } else { 2 }
}

/// Channel values allowed past the tolerance: the N64 profile's dither
/// ties round differently across GPU drivers (Metal moves a few dozen of
/// a 320×240 picture's text edges), as in `platform-pc/tests/golden.rs`.
fn allowed_bad(family: Family) -> usize {
    if family == Family::N64 { 256 } else { 0 }
}

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// The starter screens as a new project gets them (`{{name}}` filled).
fn starter(name: &str) -> ScreenDef {
    let path = repo().join(format!("templates/standalone/ui/{name}.ui.ron"));
    let text = std::fs::read_to_string(&path).unwrap();
    ScreenDef::from_ron(&text.replace("{{name}}", "orbit-cat")).unwrap()
}

fn starter_langs() -> Langs {
    let mut langs = Langs::default();
    let t = StringsDef::load(&repo().join("templates/standalone/assets/i18n/en-US.ron")).unwrap();
    langs.tables.insert(t.lang.clone(), t);
    langs
}

#[test]
fn starter_screens_match_their_goldens_on_every_profile() {
    let root = repo();
    let manifest =
        trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let langs = starter_langs();
    let golden = root.join("tests/golden/pc");
    let bless = std::env::var("TRINO_BLESS").is_ok();
    let mut failures = Vec::new();
    for screen in ["title", "pause", "options", "controls"] {
        let def = starter(screen);
        for family in [Family::N64, Family::N3ds, Family::Pc] {
            let opts = ShotOptions {
                family,
                size: Some((1920, 1080)),
                lang: Some("en-US"),
                interface_scale: 1.0,
                focus: None,
                access: trino_core::ui::UiAccess::NONE,
                simulate: None,
            };
            let shot = match ui_render::render(&root, &manifest, &langs, &def, &opts) {
                Ok(s) => s,
                Err(e) if e.starts_with("no GPU") => {
                    assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
                    eprintln!("skipping: {e}");
                    return;
                }
                Err(e) => panic!("{screen} on {}: {e}", family.key()),
            };
            let name = format!("ui_{screen}_{}", family.key());
            let path = golden.join(format!("{name}.png"));
            if bless {
                ui_render::write_png(&path, shot.width, shot.height, &shot.rgba).unwrap();
                eprintln!("blessed golden: {}", path.display());
                continue;
            }
            assert!(
                path.exists(),
                "golden missing at {} — run `cargo xtask test --bless` once and commit it",
                path.display()
            );
            let (w, h, want) = read_png(&path);
            assert_eq!((w, h), (shot.width, shot.height), "{name}: size");
            let tol = tolerance(family);
            let bad = shot
                .rgba
                .iter()
                .zip(&want)
                .filter(|(a, b)| a.abs_diff(**b) > tol)
                .count();
            if bad > allowed_bad(family) {
                let actual = golden.join(format!("actual/{name}.png"));
                ui_render::write_png(&actual, shot.width, shot.height, &shot.rgba).unwrap();
                failures.push(format!(
                    "{name}: {bad} channel values differ by more than {tol} (allowed {}, actual at {})",
                    allowed_bad(family),
                    actual.display()
                ));
            }
        }
    }
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The same screen at 1080p, 4K and 21:9: whole-pixel scaling, the panel
/// centered and the anchors at the real edges (no golden: the layout's
/// numbers are what matter).
#[test]
fn starter_screens_fill_wide_and_4k_pictures() {
    let root = repo();
    let manifest =
        trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let langs = starter_langs();
    let def = starter("title");
    for ((w, h), scale) in [((1920, 1080), 4), ((3840, 2160), 9), ((2560, 1080), 4)] {
        let opts = ShotOptions {
            family: Family::Pc,
            size: Some((w, h)),
            lang: Some("en-US"),
            interface_scale: 1.0,
            focus: None,
            access: trino_core::ui::UiAccess::NONE,
            simulate: None,
        };
        let shot = match ui_render::render(&root, &manifest, &langs, &def, &opts) {
            Ok(s) => s,
            Err(e) if e.starts_with("no GPU") => {
                assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
                return;
            }
            Err(e) => panic!("{e}"),
        };
        assert_eq!((shot.width, shot.height), (w, h));
        assert_eq!(shot.stats.overflow, 0, "{w}x{h}");
        // The focused Play button: focus yellow at the center column, at
        // the button's place for this scale (centered stack, 24 units below
        // the middle: rows of 18 with a gap of 6 → Play starts 12 units
        // above the stack's own center).
        let x = w / 2 - 40 * scale;
        let y = h / 2 + (24 - 21 + 9) * scale;
        let at = ((y * w + x) * 4) as usize;
        let px = &shot.rgba[at..at + 3];
        assert!(
            px[0] > 200 && px[1] > 170 && px[2] < 140,
            "{w}x{h}: Play is not focused at ({x}, {y}): {px:?}"
        );
    }
}
