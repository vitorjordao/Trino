//! `cargo xtask services check|list|lock|export steam` through the real
//! binary (issue #61): the engine's castle64 table passes, a fixture with
//! one problem of each kind fails with each named
//! (`tests/services_fixtures/broken`), and the Steam export writes every
//! file (written under `target/`, never the system temp dir).

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

fn json(o: &Output) -> serde_json::Value {
    serde_json::from_slice(&o.stdout).unwrap_or_else(|e| {
        panic!(
            "{e}: {}\n{}",
            String::from_utf8_lossy(&o.stdout),
            String::from_utf8_lossy(&o.stderr)
        )
    })
}

#[test]
fn services_check_passes_on_castle64() {
    let o = xtask(&["services", "check", "--json"]);
    let r = json(&o);
    assert!(o.status.success(), "{r:#}");
    assert_eq!(r["errors"], 0, "{r:#}");
    assert_eq!(r["warnings"], 0, "{r:#}");
}

#[test]
fn services_check_names_each_problem() {
    let o = xtask(&[
        "services",
        "check",
        "xtask/tests/services_fixtures/broken",
        "--json",
    ]);
    assert!(!o.status.success());
    let r = json(&o);
    let findings = r["findings"].as_array().unwrap();
    let has = |area: &str, needle: &str| {
        findings.iter().any(|f| {
            f["area"] == area && f["message"].as_str().unwrap().contains(needle)
                || f["area"] == area && f["subject"].as_str().unwrap().contains(needle)
        })
    };
    assert!(has("icons", "sprites/missing"), "{r:#}");
    assert!(has("achievements", "outside its stat's range"), "{r:#}");
    assert!(has("achievements", "letters, digits"), "{r:#}");
    assert!(has("achievements", "`NOPE` is not declared"), "{r:#}");
    assert!(has("lock", "saves would read the wrong bits"), "{r:#}");
    assert!(has("strings", "c.desc"), "{r:#}");
    assert!(has("presence", "presence.none"), "{r:#}");
}

#[test]
fn services_list_reports_the_save_cost() {
    let o = xtask(&["services", "list", "--json"]);
    assert!(o.status.success());
    let r = json(&o);
    assert_eq!(r["achievements"].as_array().unwrap().len(), 5);
    assert_eq!(r["achievements"][1]["stat"], "COINS");
    assert_eq!(r["achievements"][4]["hidden"], true);
    assert_eq!(r["save_bytes"], 12, "3 + 1 + 4 × 2");
}

#[test]
fn services_export_steam_writes_every_file() {
    let out = root().join("target/test-projects/steam-config");
    let _ = std::fs::remove_dir_all(&out);
    let o = xtask(&[
        "services",
        "export",
        "steam",
        "--out",
        out.to_str().unwrap(),
        "--json",
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let r = json(&o);
    assert_eq!(r["files"].as_array().unwrap().len(), 3 + 3 + 10);
    let csv = std::fs::read_to_string(out.join("achievements.csv")).unwrap();
    assert!(csv.starts_with("api_name,token,name_english"));
    assert!(csv.contains("FIRST_COIN,NEW_ACHIEVEMENT_1_0,First coin"));
    let pt = std::fs::read_to_string(out.join("rich_presence/brazilian.vdf")).unwrap();
    assert!(pt.contains("\"#presence_stage\"\t\"Jogando a fase %stage%\""));
    let actions = std::fs::read_to_string(out.join("game_actions_0.vdf")).unwrap();
    assert!(actions.contains("\"button_a\""));
    let icon = std::fs::File::open(out.join("icons/FIRST_COIN.png")).unwrap();
    let info = png::Decoder::new(std::io::BufReader::new(icon))
        .read_info()
        .unwrap()
        .info()
        .clone();
    assert_eq!((info.width, info.height), (64, 64));
}
