//! The post stack's tools through the real binary (issue #64, ADR 0033):
//! `lut neutral|inspect`, `post check` on the repo and on broken projects,
//! `scene export --bake-grading` and `scene shot` of the reference scene
//! (`scenes/post-reference.scene.ron`) on every platform — the consoles'
//! baked grading matching the PC's post stack where the colors are unlit.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn xtask_in(dir: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(dir)
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn xtask(args: &[&str]) -> Output {
    xtask_in(&root(), args)
}

fn text(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", text(&o.stdout), text(&o.stderr)))
}

/// A shot needs a GPU: skip without one, unless `TRINO_REQUIRE_GPU` is set.
fn gpu_missing(o: &Output) -> bool {
    let missing = !o.status.success() && text(&o.stdout).contains("no GPU");
    assert!(
        !missing || std::env::var("TRINO_REQUIRE_GPU").is_err(),
        "{}",
        text(&o.stdout)
    );
    missing
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = d.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn lut_neutral_writes_the_identity_and_inspect_reads_it() {
    let dir = root().join("target/test-projects/post-lut");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let out = dir.join("neutral.png");
    let o = xtask(&[
        "lut",
        "neutral",
        "--size",
        "16",
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let j = json(&o);
    assert_eq!(
        (j["width"].as_u64(), j["height"].as_u64()),
        (Some(256), Some(16))
    );
    let o = xtask(&["lut", "inspect", out.to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let j = json(&o);
    assert_eq!(j["size"], 16);
    assert_eq!(j["identity"], true, "{j}");
    // The repo's selftest LUT rotates the channels.
    let o = xtask(&["lut", "inspect", "luts/selftest", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let j = json(&o);
    assert_eq!(j["identity"], false);
    let s = &j["samples"][3];
    assert_eq!(s["in"], serde_json::json!([200, 60, 20]));
    assert_eq!(s["out"], serde_json::json!([60, 20, 200]));
    let o = xtask(&["lut", "neutral", "--size", "99"]);
    assert!(!o.status.success());
}

#[test]
fn post_check_passes_on_the_repo_and_names_each_broken_reference() {
    let o = xtask(&["post", "check", "--json"]);
    let j = json(&o);
    assert!(o.status.success(), "{j}");
    assert_eq!(j["errors"], 0, "{j}");
    let luts = j["luts"].as_array().unwrap();
    assert!(
        luts.iter()
            .any(|l| l["lut"] == "luts/selftest" && l["size"] == 17)
    );

    // A project whose scene names a LUT nobody declared, bakes grading
    // without one, and draws an ungraded model in a graded scene.
    let dir = root().join("target/test-projects/post-check");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("assets/shared/luts")).unwrap();
    std::fs::create_dir_all(dir.join("assets/shared/models")).unwrap();
    std::fs::create_dir_all(dir.join("scenes")).unwrap();
    std::fs::copy(
        root().join("assets/shared/luts/selftest.png"),
        dir.join("assets/shared/luts/warm.png"),
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/shared/models/box.prim.ron"),
        "(shape: Box(size: (1.0, 1.0, 1.0)), color: (200, 100, 50, 255))",
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/manifest.toml"),
        "version = 1\n\n[models.box]\nfile = \"models/box.prim.ron\"\n\n\
         [luts.warm]\nfile = \"luts/warm.png\"\n\n\
         [scenes.graded]\nfile = \"scenes/graded.scene.ron\"\nbake_grading = true\n\n\
         [scenes.plain]\nfile = \"scenes/plain.scene.ron\"\nbake_grading = true\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/graded.scene.ron"),
        "(version: 7, name: \"graded\", environment: (post: Some((grading: Some(\"luts/warm\")))), \
         entities: [(name: \"b\", model: Some((path: \"models/box\")))])",
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/plain.scene.ron"),
        "(version: 7, name: \"plain\", entities: [])",
    )
    .unwrap();
    std::fs::write(
        dir.join("scenes/missing.scene.ron"),
        "(version: 7, name: \"missing\", environment: (post: Some((grading: Some(\"luts/cold\")))), \
         entities: [])",
    )
    .unwrap();
    let o = xtask(&["post", "check", dir.to_str().unwrap(), "--json"]);
    let j = json(&o);
    assert!(!o.status.success(), "{j}");
    let found: Vec<String> = j["findings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|f| format!("{} {} {}", f["level"], f["asset"], f["message"]))
        .collect();
    let has = |s: &str| found.iter().any(|f| f.contains(s));
    assert!(has("luts/cold"), "{found:?}");
    assert!(has("bake_grading needs"), "{found:?}");
    assert!(has("draws `models/box`, graded by no LUT"), "{found:?}");
    assert_eq!(j["errors"], 2, "{found:?}");
    assert_eq!(j["warnings"], 1, "{found:?}");
}

#[test]
fn a_bake_grading_export_grades_the_scene_colors_for_the_consoles() {
    let dir = root().join("target/test-projects/post-export");
    std::fs::create_dir_all(&dir).unwrap();
    let pc = dir.join("pc.rs");
    let n64 = dir.join("n64.rs");
    let scene = "scenes/post-reference.scene.ron";
    let o = xtask(&["scene", "export", scene, pc.to_str().unwrap()]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let o = xtask(&[
        "scene",
        "export",
        "--platform",
        "n64",
        "--bake-grading",
        scene,
        n64.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let (pc, n64) = (
        std::fs::read_to_string(pc).unwrap(),
        std::fs::read_to_string(n64).unwrap(),
    );
    // The clear color (40, 60, 110) rotated, on the console's only.
    assert!(pc.contains("Color::rgb(40, 60, 110)"), "{pc}");
    assert!(n64.contains("Color::rgb(60, 110, 40)"), "{n64}");
    // The PC keeps the post stack; the lamp's glow is in both.
    assert!(pc.contains("pub const POST: Option<PostSettings> = Some(PostSettings"));
    assert!(pc.contains("glow: Some(Glow {") && n64.contains("glow: Some(Glow {"));
}

/// The reference scene on the PC (the post stack grades the frame) and on
/// each console (the bake graded the card and the background): the same
/// colors where they are unlit. The lit card differs by the order of
/// grading and lighting, measured and bounded.
#[test]
fn the_reference_scene_grades_alike_on_the_pc_and_the_consoles() {
    let dir = root().join("target/test-projects/post-shots");
    std::fs::create_dir_all(&dir).unwrap();
    let mut shots = Vec::new();
    for platform in ["pc", "n64", "3ds"] {
        let out = dir.join(format!("{platform}.png"));
        let mut args = vec![
            "scene",
            "shot",
            "scenes/post-reference.scene.ron",
            "--platform",
            platform,
            "--out",
            out.to_str().unwrap(),
            "--json",
        ];
        if platform == "pc" {
            args.extend(["--size", "320x240"]);
        }
        let o = xtask(&args);
        if gpu_missing(&o) {
            return;
        }
        assert!(
            o.status.success(),
            "{}\n{}",
            text(&o.stdout),
            text(&o.stderr)
        );
        let j = json(&o);
        if platform == "pc" {
            assert_eq!(j["post_passes"], 1, "{j}");
            assert_eq!(j["glows"], 0, "the PC blooms instead: {j}");
        } else {
            assert_eq!(j["post_passes"], 0, "{j}");
            assert_eq!(j["glows"], 1, "{j}");
            assert_eq!(j["graded_environment"], true, "{j}");
        }
        shots.push((platform, read_png(&out)));
    }
    // Points as pixels from the center of 240-line frames (the 3DS's is
    // 400 wide: the projection keeps pixel offsets, not fractions).
    let probe = |(w, h, px): &(u32, u32, Vec<u8>), dx: i32, dy: i32| {
        let x = (*w as i32 / 2 + dx) as u32;
        let y = (*h as i32 / 2 + dy) as u32;
        let i = ((y * w + x) * 4) as usize;
        [px[i], px[i + 1], px[i + 2]]
    };
    let (_, pc) = &shots[0];
    for (platform, img) in &shots[1..] {
        // Background (top corner) and the two unlit cards' centers.
        for (what, dx, dy, tol) in [
            ("background", -140, -100, 12u8),
            ("left card", -70, -10, 14),
            ("right card", 70, -10, 14),
        ] {
            let (a, b) = (probe(pc, dx, dy), probe(img, dx, dy));
            for k in 0..3 {
                assert!(
                    a[k].abs_diff(b[k]) <= tol,
                    "{platform} {what}: pc {a:?} vs {b:?}"
                );
            }
        }
    }
    // And the PC without its post stack is ungraded: the clear color as
    // authored.
    let off = dir.join("pc-off.png");
    let o = xtask(&[
        "scene",
        "shot",
        "scenes/post-reference.scene.ron",
        "--size",
        "320x240",
        "--post",
        "off",
        "--out",
        off.to_str().unwrap(),
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let bg = probe(&read_png(&off), -140, -100);
    assert!(bg[2] > bg[0] + 40, "authored blue background: {bg:?}");
}
