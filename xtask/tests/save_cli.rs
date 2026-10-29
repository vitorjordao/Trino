//! `cargo xtask save` and `cargo xtask fuzz` through the real binary
//! (issue #68): what an agent runs to look inside a save, damage one, and
//! replay a fuzz input.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

use trino_core::save::{self, Lz, Medium};

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

/// A scratch folder under target/test-projects (never the system temp dir).
fn scratch(name: &str) -> PathBuf {
    let d = root()
        .join("target/test-projects")
        .join(format!("save-cli-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn slot(path: &Path, seq: u16, data: &[u8]) {
    let mut b = vec![0u8; 256];
    let n = save::encode_slot(seq, data, &mut b).unwrap();
    std::fs::write(path, &b[..n]).unwrap();
}

#[test]
fn inspect_and_corrupt_a_pc_slot_pair() {
    let d = scratch("pc");
    let base = d.join("game.sav");
    slot(&d.join("game.sav.a"), 1, b"coins=3");
    slot(&d.join("game.sav.b"), 2, b"coins=9");
    let base_s = base.to_str().unwrap();
    let o = xtask(&["save", "inspect", base_s, "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    assert_eq!(r["kind"], "slots");
    assert_eq!(r["status"], "ok");
    assert_eq!(r["newest"], "game.sav.b");
    assert_eq!(r["len"], 7);
    assert_eq!(r["preview"], "coins=9");
    assert_eq!(r["slots"][1]["seq"], 2);
    // Bit rot in the newest: the previous save, reported as recovered.
    let o = xtask(&["save", "corrupt", base_s, "--mode", "flip"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&xtask(&["save", "inspect", base_s, "--json"]));
    assert_eq!(r["status"], "recovered-from-backup");
    assert_eq!(r["newest"], "game.sav.a");
    assert_eq!(r["slots"][1]["state"], "damaged");
    // One slot cut short by hand, then everything damaged.
    let o = xtask(&[
        "save", "corrupt", base_s, "--mode", "truncate", "--slot", "a", "--at", "20",
    ]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(std::fs::read(d.join("game.sav.a")).unwrap().len(), 20);
    let r = json(&xtask(&["save", "inspect", base_s, "--json"]));
    assert_eq!(r["status"], "corrupted");
    assert_eq!(r["len"], 0);
    // The text report names the same.
    let o = xtask(&["save", "inspect", base_s]);
    assert!(
        text(&o.stdout).contains("status: corrupted"),
        "{}",
        text(&o.stdout)
    );
    let _ = std::fs::remove_dir_all(&d);
}

struct Image(Vec<u8>);

impl Medium for Image {
    fn size(&self) -> usize {
        self.0.len()
    }
    fn read(&mut self, at: usize, buf: &mut [u8]) {
        buf.copy_from_slice(&self.0[at..at + buf.len()]);
    }
    fn write(&mut self, at: usize, data: &[u8]) {
        self.0[at..at + data.len()].copy_from_slice(data);
    }
}

#[test]
fn inspect_and_corrupt_an_eeprom_image() {
    let d = scratch("eeprom");
    let file = d.join("castle64.eeprom");
    let mut m = Image(vec![0xFF; 512]);
    let mut body = vec![0u8; 512];
    let mut scratch_buf = vec![0u8; 512];
    for data in [&b"stage=1"[..], b"stage=2"] {
        save::write_now(
            &mut m,
            data,
            &mut body,
            &mut scratch_buf,
            &mut Lz::default(),
        )
        .unwrap();
    }
    std::fs::write(&file, &m.0).unwrap();
    let f = file.to_str().unwrap();
    let r = json(&xtask(&["save", "inspect", f, "--json"]));
    assert_eq!(r["kind"], "medium");
    assert_eq!(
        (r["status"].as_str(), r["preview"].as_str()),
        (Some("ok"), Some("stage=2"))
    );
    assert_eq!(r["slots"].as_array().unwrap().len(), 2);
    assert!(
        xtask(&["save", "corrupt", f, "--mode", "truncate"])
            .status
            .success()
    );
    let r = json(&xtask(&["save", "inspect", f, "--json"]));
    assert_eq!(
        (r["status"].as_str(), r["preview"].as_str()),
        (Some("recovered-from-backup"), Some("stage=1"))
    );
    let _ = std::fs::remove_dir_all(&d);
}

/// Azahar's SD card (issues #119, #121): a game's folder that holds no
/// slot file reads the shared folder's save (the one-time migration), then
/// its own; the shared folder reads `save.bin` from before the slots. The
/// editor's Saves tab lists the same folders (`saves::n3ds_folders`).
#[test]
fn inspect_a_3ds_sd_folder_and_its_migration() {
    let sd = scratch("sd");
    let shared = sd.join("3ds/trino");
    let game = shared.join("castle64");
    std::fs::create_dir_all(&game).unwrap();
    slot(&shared.join("save.a"), 3, b"shared-save");
    let mut bin = save::header(b"older").to_vec();
    bin.extend_from_slice(b"older");
    std::fs::write(shared.join("save.bin"), &bin).unwrap();

    let r = json(&xtask(&[
        "save",
        "inspect",
        game.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(r["kind"], "3ds folder");
    assert_eq!(r["status"], "ok");
    assert_eq!(r["preview"], "shared-save");
    assert_eq!(r["newest"], "the shared folder (migrates once)");
    assert_eq!(r["slots"][2]["state"], "shared");

    // Once the game saved, its own folder wins.
    slot(&game.join("save.b"), 1, b"own-save");
    let r = json(&xtask(&[
        "save",
        "inspect",
        game.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(
        (r["preview"].as_str(), r["newest"].as_str()),
        (Some("own-save"), Some("save.b"))
    );
    assert_eq!(r["slots"].as_array().unwrap().len(), 2, "no shared lines");

    // A damaged own slot never falls back to the shared save.
    let mut b = std::fs::read(game.join("save.b")).unwrap();
    let last = b.len() - 1;
    b[last] ^= 1;
    std::fs::write(game.join("save.b"), &b).unwrap();
    let r = json(&xtask(&[
        "save",
        "inspect",
        game.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(
        (r["status"].as_str(), r["len"].as_u64()),
        (Some("corrupted"), Some(0))
    );

    // The shared folder: its slot, and save.bin listed as legacy.
    let r = json(&xtask(&[
        "save",
        "inspect",
        shared.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(r["kind"], "3ds shared folder");
    assert_eq!(r["preview"], "shared-save");
    assert_eq!(r["slots"][2]["name"], "save.bin");
    std::fs::remove_file(shared.join("save.a")).unwrap();
    let r = json(&xtask(&[
        "save",
        "inspect",
        shared.to_str().unwrap(),
        "--json",
    ]));
    assert_eq!(
        (r["preview"].as_str(), r["newest"].as_str()),
        (Some("older"), Some("save.bin"))
    );

    let folders = trino_asset_pipeline::saves::n3ds_folders(&sd);
    assert_eq!(folders, vec![shared.clone(), game.clone()]);
    std::fs::write(sd.join("x.eeprom"), [0u8; 512]).unwrap();
    assert_eq!(
        trino_asset_pipeline::saves::ares_eeproms(&sd),
        vec![sd.join("x.eeprom")]
    );
    let _ = std::fs::remove_dir_all(&sd);
}

#[test]
fn bad_save_invocations_fail_with_a_reason() {
    let o = xtask(&["save"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("usage: cargo xtask save inspect"),
        "{}",
        text(&o.stderr)
    );
    let o = xtask(&["save", "inspect", "target/test-projects/no/such.sav"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("no/such.sav"),
        "{}",
        text(&o.stderr)
    );
    let d = scratch("bad");
    slot(&d.join("x.sav.a"), 1, b"x");
    let o = xtask(&[
        "save",
        "corrupt",
        d.join("x.sav").to_str().unwrap(),
        "--mode",
        "melt",
    ]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("--mode flip|truncate"),
        "{}",
        text(&o.stderr)
    );
    let _ = std::fs::remove_dir_all(&d);
}

#[test]
fn fuzz_lists_every_target_and_replays_a_regression() {
    let o = xtask(&["fuzz", "list", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    let r = json(&o);
    let names: Vec<&str> = r["targets"]
        .as_array()
        .unwrap()
        .iter()
        .map(|t| t["name"].as_str().unwrap())
        .collect();
    for want in ["tmdl", "tscn", "save_frame", "input_text", "pc_bundle"] {
        assert!(names.contains(&want), "{names:?}");
    }
    assert!(
        r["targets"]
            .as_array()
            .unwrap()
            .iter()
            .all(|t| t["seeds"].as_u64().unwrap() > 0)
    );
    // A crash reproducer replays clean (the fix holds).
    let file = std::fs::read_dir(root().join("fuzz/regressions/tmdl"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let o = xtask(&["fuzz", "repro", file.to_str().unwrap(), "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(json(&o)["ok"], true);
    // A short smoke run of one target.
    let o = xtask(&["fuzz", "smoke", "tpfx", "--iters", "200", "--json"]);
    assert!(o.status.success(), "{}", text(&o.stderr));
    assert_eq!(json(&o)["ok"], true);
    // Bad invocations say why.
    let o = xtask(&["fuzz", "melt"]);
    assert!(!o.status.success());
    assert!(
        text(&o.stderr).contains("unknown command `melt`"),
        "{}",
        text(&o.stderr)
    );
    let o = xtask(&["fuzz", "repro", "Cargo.toml"]);
    assert!(!o.status.success());
    let o = xtask(&["fuzz", "keep", "Cargo.toml"]);
    assert!(!o.status.success());
}
