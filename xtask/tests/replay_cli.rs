//! `cargo xtask replay` and `cargo xtask net` through the real binary
//! (issue #67): what an agent runs on a recording — dump it as JSON, replay
//! it on the PC headless (and catch a tampered copy), check it — and the
//! `[players]` / `[net]` keys of a project's `trino.toml`.

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

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}\n{}", text(&o.stdout), text(&o.stderr)))
}

const CASTLE: &str = "assets/shared/replays/castle64_green.trec";
const TWO: &str = "examples/solid-eleven/assets/shared/replays/two_players.trec";

#[test]
fn dump_reads_a_recording_as_json() {
    let o = xtask(&["replay", "dump", CASTLE, "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    assert_eq!(r["header"]["game"], "castle64");
    assert_eq!(r["header"]["frames"], 731);
    assert_eq!(r["header"]["hash_every"], 1);
    let f = &r["frames"][100];
    assert_eq!(f["frame"], 100);
    assert!(f["hash"].as_str().unwrap().starts_with("0x"));
    assert!(f["players"][0]["stick"].is_array());
    // Two players, the menus' presses by name.
    let r = json(&xtask(&["replay", "dump", TWO, "--json"]));
    assert_eq!(r["header"]["players"], 2);
    assert_eq!(r["frames"][10]["players"][0]["buttons"][0], "A");
    // Text: one line per change.
    let o = xtask(&["replay", "dump", TWO]);
    assert!(
        text(&o.stdout).contains("2 player(s)"),
        "{}",
        text(&o.stdout)
    );
}

#[test]
fn run_replays_on_the_pc_and_catches_a_tampered_copy() {
    let o = xtask(&["replay", "run", CASTLE, "--platform", "pc", "--json"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let r = json(&o);
    assert_eq!(
        (r["ok"].as_bool(), r["frames"].as_u64()),
        (Some(true), Some(731))
    );
    assert_eq!(r["checked"], 731);
    // Stopped early.
    let r = json(&xtask(&[
        "replay",
        "run",
        CASTLE,
        "--platform",
        "pc",
        "--until-frame",
        "100",
        "--json",
    ]));
    assert_eq!(r["frames"], 100);
    // A bit flipped in the middle of the frames: the header stays valid,
    // a frame's input (or its encoding) changes, and the run fails.
    let mut bytes = std::fs::read(root().join(CASTLE)).unwrap();
    let at = bytes.len() / 2;
    bytes[at] ^= 0x10;
    let dir = tempfile::tempdir_in(root().join("target")).unwrap();
    let bad = dir.path().join("bad.trec");
    std::fs::write(&bad, &bytes).unwrap();
    let o = xtask(&[
        "replay",
        "run",
        bad.to_str().unwrap(),
        "--platform",
        "pc",
        "--json",
    ]);
    assert!(!o.status.success(), "a tampered recording passed");
    let r: serde_json::Value = serde_json::from_slice(&o.stdout).unwrap_or_default();
    assert!(
        r["diverged"].is_object() || r["error"].is_string(),
        "{}",
        text(&o.stdout)
    );
}

#[test]
fn check_holds_every_platform_to_one_hash() {
    let o = xtask(&["replay", "check", TWO, "--platforms", "pc", "--json"]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let r = json(&o);
    assert_eq!(r["ok"], true);
    assert_eq!(r["recordings"][0]["runs"][0]["platform"], "pc");
    assert_eq!(r["recordings"][0]["runs"][0]["frames"], 640);
}

#[test]
fn net_config_reads_and_writes_trino_toml() {
    let dir = tempfile::tempdir_in(root().join("target")).unwrap();
    std::fs::write(
        dir.path().join("trino.toml"),
        "[project]\nname = \"netty\"\ntitle = \"Netty\" # kept\nplatforms = [\"pc\"]\n",
    )
    .unwrap();
    let p = dir.path().to_str().unwrap();
    let o = xtask(&[
        "net",
        "config",
        "set",
        "players.max=2",
        "net.mode=delay",
        "--project",
        p,
    ]);
    assert!(o.status.success(), "{}{}", text(&o.stdout), text(&o.stderr));
    let t = std::fs::read_to_string(dir.path().join("trino.toml")).unwrap();
    assert!(
        t.contains("# kept") && t.contains("mode = \"delay\""),
        "{t}"
    );
    let r = json(&xtask(&["net", "config", "--json", "--project", p]));
    assert_eq!(r["players"]["max"], 2);
    assert_eq!(r["net"]["mode"], "delay");
    let o = xtask(&[
        "net",
        "config",
        "set",
        "net.max_rollback=99",
        "--project",
        p,
    ]);
    assert!(!o.status.success(), "out of range accepted");
    // The engine itself: the defaults.
    let r = json(&xtask(&["net", "config", "--json"]));
    assert_eq!(r["declared"], false);
    assert_eq!(r["net"]["max_rollback"], 8);
}
