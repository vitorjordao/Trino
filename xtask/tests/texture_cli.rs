//! `cargo xtask texture inspect` through the real binary: the engine's own
//! self-test cards (one per 3DS format) and the usage on bad invocations.

use std::path::PathBuf;
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

#[test]
fn inspect_reports_format_bytes_psnr_and_memory_per_platform() {
    let o = xtask(&["texture", "inspect", "sprites/selftest_fmt_etc1", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    assert_eq!(doc["texture"], "sprites/selftest_fmt_etc1");
    let platforms = doc["platforms"].as_array().unwrap();
    let find = |key: &str| {
        platforms
            .iter()
            .find(|p| p["platform"] == key)
            .unwrap_or_else(|| panic!("{key} missing"))
    };
    // The PC is `auto`: flat 16x16 quadrants are lossless in BC1.
    let pc = find("pc");
    assert_eq!(pc["format"], "BC1");
    assert_eq!(pc["bytes"], 32 * 32 / 2);
    assert_eq!(pc["lossless"], true);
    assert_eq!(pc["auto"], true);
    assert_eq!(pc["memory"], "VRAM");
    let measured: Vec<&str> = pc["candidates"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["format"].as_str().unwrap())
        .collect();
    assert_eq!(measured, ["BC1", "BC3", "BC7", "RGBA8"]);
    assert!(
        pc["reason"]
            .as_str()
            .unwrap()
            .starts_with("auto (opaque color")
    );
    let n64 = find("n64");
    assert_eq!(n64["format"], "RGBA5551");
    assert_eq!(n64["memory"], "TMEM");
    assert_eq!(n64["memory_bytes"], 2048);
    assert_eq!(n64["memory_percent"], 50.0);
    let n3ds = find("3ds");
    assert_eq!(n3ds["format"], "ETC1");
    assert_eq!(
        (n3ds["auto"].as_bool(), n3ds["reason"].as_str()),
        (Some(false), Some("declared"))
    );
    assert_eq!(n3ds["bytes"], 32 * 32 / 2, "4 bits a texel");
    assert_eq!(n3ds["memory_budget"], 6 * 1024 * 1024);
    // Solid 16×16 quadrants: ETC1 keeps them almost exactly.
    assert!(n3ds["psnr"].as_f64().unwrap() > 40.0, "{n3ds}");

    // One platform, as text.
    let o = xtask(&[
        "texture",
        "inspect",
        "sprites/selftest_fmt_a8",
        "--platform",
        "3ds",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let out = text(&o.stdout);
    assert!(out.contains("3ds  A8") && out.contains("1024 B"), "{out}");
    assert!(out.contains("why: declared"), "{out}");
    assert!(!out.contains("n64"), "{out}");

    // An `auto` texture lists what it measured.
    let o = xtask(&[
        "texture",
        "inspect",
        "sprites/tex_water",
        "--platform",
        "3ds",
    ]);
    let out = text(&o.stdout);
    assert!(
        out.contains("why: auto (") && out.contains("measured: ETC1 "),
        "{out}"
    );
}

#[test]
fn report_lists_every_texture_biggest_first_with_totals() {
    let o = xtask(&["texture", "report", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let doc: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap();
    let textures = doc["textures"].as_array().unwrap();
    // Every sprite and font atlas, the per-language ones (issue #59) too.
    let manifest =
        trino_asset_pipeline::Manifest::load(&root().join("assets/manifest.toml")).unwrap();
    let declared = manifest.sprites.len() + manifest.fonts.len();
    assert_eq!(textures.len(), declared);
    // Biggest first: the largest platform size never grows down the list.
    let size = |t: &serde_json::Value| {
        t["platforms"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(|p| p["bytes"].as_u64())
            .max()
            .unwrap()
    };
    assert!(textures.windows(2).all(|w| size(&w[0]) >= size(&w[1])));
    // The totals: the 3DS takes less linear memory than RGBA8 would, and
    // `auto` picked most of the PC's formats.
    let totals = doc["totals"].as_array().unwrap();
    let t3ds = totals.iter().find(|t| t["platform"] == "3ds").unwrap();
    assert!(
        t3ds["bytes"].as_u64().unwrap() * 2 < t3ds["rgba8_bytes"].as_u64().unwrap(),
        "{t3ds}"
    );
    let tpc = totals.iter().find(|t| t["platform"] == "pc").unwrap();
    assert!(
        tpc["auto"].as_u64().unwrap() * 2 > textures.len() as u64,
        "{tpc}"
    );
    assert_eq!(
        totals.iter().find(|t| t["platform"] == "n64").unwrap()["rgba8_bytes"],
        serde_json::Value::Null
    );

    // Text, one platform.
    let o = xtask(&["texture", "report", "--platform", "3ds"]);
    let out = text(&o.stdout);
    assert!(
        out.contains("textures, biggest first") && out.contains("total 3ds"),
        "{out}"
    );
    assert!(!out.contains(" pc "), "{out}");
}

#[test]
fn bad_texture_invocations_fail_with_the_usage_or_a_reason() {
    let o = xtask(&["texture"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("texture inspect"));
    assert!(text(&o.stderr).contains("texture report"));
    let o = xtask(&["texture", "report", "--sideways"]);
    assert!(!o.status.success() && text(&o.stderr).contains("unknown argument"));
    let o = xtask(&["texture", "inspect", "sprites/nope"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("not in the manifest"),
        "{}",
        text(&o.stderr)
    );
    let o = xtask(&[
        "texture",
        "inspect",
        "sprites/selftest_fmt_a8",
        "--platform",
        "gc",
    ]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("--platform"));
}
