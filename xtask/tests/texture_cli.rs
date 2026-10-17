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
    let pc = find("pc");
    assert_eq!(pc["format"], "RGBA8");
    assert_eq!(pc["bytes"], 32 * 32 * 4);
    assert_eq!(pc["lossless"], true);
    let n64 = find("n64");
    assert_eq!(n64["format"], "RGBA5551");
    assert_eq!(n64["memory"], "TMEM");
    assert_eq!(n64["memory_bytes"], 2048);
    assert_eq!(n64["memory_percent"], 50.0);
    let n3ds = find("3ds");
    assert_eq!(n3ds["format"], "ETC1");
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
    assert!(!out.contains("n64"), "{out}");
}

#[test]
fn bad_texture_invocations_fail_with_the_usage_or_a_reason() {
    let o = xtask(&["texture"]);
    assert!(!o.status.success());
    assert!(text(&o.stderr).contains("texture inspect"));
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
