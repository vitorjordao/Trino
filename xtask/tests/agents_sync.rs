//! The AI-agent skill mirrors (`.claude/skills/`, ...) must stay byte-identical
//! to the canonical `.agents/skills/` tree. Fix a failure with
//! `cargo xtask agents sync` — never by editing a mirror.

#[path = "../src/agents.rs"]
#[allow(dead_code)]
mod agents;

use std::path::PathBuf;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

#[test]
fn skill_mirrors_match_canonical_tree() {
    if let Err(e) = agents::check(&root()) {
        panic!("{e}");
    }
}

#[test]
fn every_skill_is_listed_in_agents_md() {
    let guide = std::fs::read_to_string(root().join("AGENTS.md")).unwrap();
    for entry in std::fs::read_dir(root().join(agents::CANONICAL)).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            let name = path.file_name().unwrap().to_string_lossy().to_string();
            assert!(
                guide.contains(&format!("`{name}`")),
                "skill `{name}` is not listed in the AGENTS.md skills table"
            );
        }
    }
}
