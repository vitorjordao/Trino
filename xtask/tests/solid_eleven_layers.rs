//! Solid Eleven's structure, enforced (issue #39 asked for decoupled
//! `sim` / `game` / `render` / `ui` layers; a game crate may depend only on
//! `trino-core`, so they are modules of one crate and this test keeps them
//! apart):
//!
//! - `sim/` is pure fixed point: it never imports `trino_core` (floats,
//!   handles, renderers) or the other layers, and uses no `f32` outside
//!   `fx.rs`'s conversions — the determinism every target relies on.
//! - `render/` and `ui/` never reach into the sim's internals to change
//!   them: no `&mut Match` outside `game/`.
//! - The fonts bake exactly the characters the code writes
//!   (`ui::FONT_CHARS`), and `render/clips_gen.rs` is what
//!   `tools/blender/solid_eleven_anim.py` generates.

use std::path::{Path, PathBuf};

fn game() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../examples/solid-eleven")
}

fn sources(dir: &Path) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            out.extend(sources(&p));
        } else if p.extension().is_some_and(|x| x == "rs") {
            out.push((p.clone(), std::fs::read_to_string(&p).unwrap()));
        }
    }
    out
}

/// Code lines only (comments and doc comments dropped).
fn code(text: &str) -> impl Iterator<Item = (usize, &str)> {
    text.lines()
        .enumerate()
        .map(|(n, l)| (n + 1, l.split("//").next().unwrap_or("")))
        .filter(|(_, l)| !l.trim().is_empty())
}

#[test]
fn the_sim_layer_is_pure_fixed_point() {
    let mut bad = Vec::new();
    // Modules `sim/mod.rs` declares under `#[cfg(test)]` (tests, balance).
    let decl = std::fs::read_to_string(game().join("src/sim/mod.rs")).unwrap();
    let lines: Vec<&str> = decl.lines().map(str::trim).collect();
    let test_only: Vec<String> = lines
        .windows(2)
        .filter(|w| w[0] == "#[cfg(test)]")
        .filter_map(|w| w[1].strip_prefix("mod ")?.strip_suffix(';'))
        .map(|m| format!("{m}.rs"))
        .collect();
    assert!(test_only.iter().any(|m| m == "tests.rs"));
    for (path, text) in sources(&game().join("src/sim")) {
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        let in_tests = test_only.contains(&name);
        let mut test_mod = false;
        for (n, l) in code(&text) {
            if l.contains("#[cfg(test)]") {
                test_mod = true;
            }
            for banned in ["trino_core", "crate::render", "crate::ui", "crate::game"] {
                if l.contains(banned) {
                    bad.push(format!("{name}:{n}: {banned}"));
                }
            }
            // Floats: only fx.rs converts, and tests may print.
            let float = l.contains("f32") || l.contains("f64");
            if float && name != "fx.rs" && !in_tests && !test_mod {
                bad.push(format!("{name}:{n}: a float in the sim"));
            }
        }
    }
    assert!(
        bad.is_empty(),
        "sim/ must stay pure fixed point:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn render_and_ui_never_mutate_the_match() {
    let mut bad = Vec::new();
    for dir in ["src/render", "src/ui"] {
        for (path, text) in sources(&game().join(dir)) {
            for (n, l) in code(&text) {
                if l.contains("&mut Match") || l.contains("m.step(") {
                    bad.push(format!("{}:{n}", path.display()));
                }
            }
        }
    }
    assert!(
        bad.is_empty(),
        "the match changes only in game/:\n  {}",
        bad.join("\n  ")
    );
}

#[test]
fn the_fonts_bake_the_characters_the_code_writes() {
    // `chars = "auto"` (issue #59): the strings' glyphs plus `extra_chars`,
    // resolved by the manifest loader. The set must stay ui::FONT_CHARS,
    // which names and scores are uppercased into.
    let manifest =
        trino_asset_pipeline::Manifest::load(&game().join("assets/manifest.toml")).unwrap();
    let sorted = |s: &str| {
        let mut v: Vec<char> = s.chars().collect();
        v.sort();
        v.dedup();
        v.into_iter().collect::<String>()
    };
    let wanted = sorted(solid_eleven::ui::FONT_CHARS);
    for font in ["se_ui", "se_big", "se_huge"] {
        let chars = manifest.fonts[font].chars.as_deref().unwrap();
        assert_eq!(
            sorted(chars),
            wanted,
            "fonts/{font} must bake ui::FONT_CHARS exactly"
        );
    }
}

#[test]
fn the_pose_tables_match_the_clip_source() {
    let script =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../tools/blender/solid_eleven_anim.py");
    let generated = game().join("src/render/clips_gen.rs");
    let out = std::process::Command::new("python3")
        .arg(&script)
        .arg("--check")
        .arg(&generated)
        .output();
    let Ok(out) = out else {
        eprintln!("python3 not found: skipped");
        return;
    };
    assert!(
        out.status.success(),
        "{}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
}

/// The players' kick layers (issue #136): `src/render/player.tang` is what
/// `assets/anim/player.anim.ron` bakes against the QA rig, the rig's joints
/// are the ones `render::graph::RIG_JOINTS` maps (the masks count rig
/// joints), and the game reads the layers it expects.
#[test]
fn the_players_graph_is_its_bake_and_the_rig_joints_match() {
    use solid_eleven::render::anim::joint;
    use solid_eleven::render::graph::{KickLayers, RIG_JOINTS};
    use trino_asset_pipeline::{Manifest, Platform, anim_graph};
    let root = game();
    let manifest = Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let def =
        trino_scene::anim::AnimGraphDef::load(&root.join("assets/anim/player.anim.ron")).unwrap();
    let baked = anim_graph::bake(&root, &manifest, Platform::Pc, &def)
        .unwrap()
        .expect("the rig bakes on the PC");
    let committed = std::fs::read(root.join("src/render/player.tang")).unwrap();
    assert!(
        baked == committed,
        "src/render/player.tang is stale: cargo xtask anim export player --dir examples/solid-eleven --out examples/solid-eleven/src/render/player.tang"
    );
    // Its exported tables, which the game plays (issue #138), are fresh
    // and read the same layers as the blob.
    let rust = trino_scene::anim::tables_to_rust(&baked, "PLAYER_GRAPH", "player").unwrap();
    let committed = std::fs::read_to_string(root.join("src/render/player_graph.rs")).unwrap();
    assert!(
        rust == committed,
        "src/render/player_graph.rs is stale: cargo xtask anim export player --dir examples/solid-eleven --rust examples/solid-eleven/src/render/player_graph.rs --static PLAYER_GRAPH"
    );
    let blob = trino_core::AnimGraph::parse(&baked).unwrap();
    assert_eq!(KickLayers::of(&blob), Some(KickLayers::load()));
    let joints = anim_graph::model_joints(&root, &manifest, Platform::Pc, "models/se_player_rig")
        .unwrap()
        .unwrap();
    let names: Vec<&str> = joints.iter().map(|j| j.name.as_str()).collect();
    let mapped: Vec<&str> = RIG_JOINTS.iter().map(|(n, _)| *n).collect();
    assert_eq!(names, mapped);
    let mut game: Vec<usize> = RIG_JOINTS.iter().map(|(_, j)| *j).collect();
    game.sort();
    assert_eq!(game, (0..joints.len()).collect::<Vec<_>>());
    let bit = |j: usize| 1u16 << j;
    let arms =
        bit(joint::ARM_L) | bit(joint::ARM_R) | bit(joint::FOREARM_L) | bit(joint::FOREARM_R);
    assert_eq!(
        KickLayers::load(),
        KickLayers {
            strike: !arms & ((1 << joints.len()) - 1),
            arms,
            clips: 0b1_1111,
        }
    );
}
