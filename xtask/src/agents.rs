//! `cargo xtask agents <sync|check>` — keep AI-agent tooling vendor-neutral.
//!
//! The canonical skills live in `.agents/skills/<name>/` (the Agent Skills
//! open-standard layout: a folder with a `SKILL.md` carrying `name` +
//! `description` frontmatter). Agents that read `.agents/skills/` natively
//! (Codex, Copilot, Cursor, Gemini CLI, ...) need nothing else; tools with a
//! private folder get a **byte-identical copy**:
//!
//! - `.claude/skills/` — Claude Code.
//!
//! Copies instead of symlinks on purpose: Windows checkouts do not reliably
//! materialize symlinks. `xtask/tests/agents_sync.rs` fails CI when a mirror
//! drifts from the canonical tree, so edits always go to `.agents/skills/`
//! followed by `cargo xtask agents sync`.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::ExitCode;

/// Canonical skills folder, relative to the repo root.
pub const CANONICAL: &str = ".agents/skills";
/// Generated mirrors, relative to the repo root.
pub const MIRRORS: &[&str] = &[".claude/skills"];

pub fn run(root: &Path, sub: Option<&str>) -> ExitCode {
    match sub {
        Some("sync") => match sync(root) {
            Ok(n) => {
                println!("agents: mirrored {n} file(s) into {}", MIRRORS.join(", "));
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("agents: {e}");
                ExitCode::FAILURE
            }
        },
        Some("check") => match check(root) {
            Ok(()) => {
                println!("agents: mirrors are in sync");
                ExitCode::SUCCESS
            }
            Err(e) => {
                eprintln!("agents: {e}");
                ExitCode::FAILURE
            }
        },
        _ => {
            eprintln!("xtask: usage: cargo xtask agents <sync|check>");
            ExitCode::FAILURE
        }
    }
}

/// Every file under `dir`, keyed by its path relative to `dir`.
fn tree(dir: &Path) -> Result<BTreeMap<PathBuf, Vec<u8>>, String> {
    let mut out = BTreeMap::new();
    if !dir.exists() {
        return Ok(out);
    }
    let mut stack = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let entries =
            std::fs::read_dir(&d).map_err(|e| format!("cannot read {}: {e}", d.display()))?;
        for entry in entries {
            let path = entry.map_err(|e| e.to_string())?.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                let bytes = std::fs::read(&path)
                    .map_err(|e| format!("cannot read {}: {e}", path.display()))?;
                out.insert(path.strip_prefix(dir).unwrap().to_path_buf(), bytes);
            }
        }
    }
    Ok(out)
}

/// Validate the canonical skills: each folder has a `SKILL.md` whose
/// frontmatter `name` matches the folder and carries a `description`.
pub fn validate(root: &Path) -> Result<(), String> {
    let dir = root.join(CANONICAL);
    let entries =
        std::fs::read_dir(&dir).map_err(|e| format!("cannot read {}: {e}", dir.display()))?;
    for entry in entries {
        let path = entry.map_err(|e| e.to_string())?.path();
        if !path.is_dir() {
            continue;
        }
        let folder = path.file_name().unwrap().to_string_lossy().to_string();
        let skill = path.join("SKILL.md");
        let text = std::fs::read_to_string(&skill)
            .map_err(|e| format!("{}: {e} (every skill needs a SKILL.md)", skill.display()))?
            .replace("\r\n", "\n"); // tolerate CRLF checkouts
        let front = text
            .strip_prefix("---\n")
            .and_then(|t| t.split_once("\n---"))
            .map(|(f, _)| f)
            .ok_or_else(|| format!("{}: missing `---` frontmatter", skill.display()))?;
        let field = |key: &str| {
            front.lines().find_map(|l| {
                l.strip_prefix(key)
                    .and_then(|r| r.strip_prefix(':'))
                    .map(|v| v.trim().to_string())
            })
        };
        match field("name") {
            Some(name) if name == folder => {}
            other => {
                return Err(format!(
                    "{}: frontmatter name {other:?} must equal the folder name `{folder}`",
                    skill.display()
                ));
            }
        }
        if field("description").is_none_or(|d| d.is_empty()) {
            return Err(format!(
                "{}: frontmatter needs a description",
                skill.display()
            ));
        }
    }
    Ok(())
}

/// Rewrite every mirror to match the canonical tree (stale files removed).
pub fn sync(root: &Path) -> Result<usize, String> {
    validate(root)?;
    let canonical = tree(&root.join(CANONICAL))?;
    for mirror in MIRRORS {
        let dir = root.join(mirror);
        if dir.exists() {
            std::fs::remove_dir_all(&dir)
                .map_err(|e| format!("cannot clear {}: {e}", dir.display()))?;
        }
        for (rel, bytes) in &canonical {
            let dst = dir.join(rel);
            std::fs::create_dir_all(dst.parent().unwrap())
                .map_err(|e| format!("cannot create {}: {e}", dst.display()))?;
            std::fs::write(&dst, bytes)
                .map_err(|e| format!("cannot write {}: {e}", dst.display()))?;
        }
    }
    Ok(canonical.len() * MIRRORS.len())
}

/// Error listing every drifted path, or Ok when all mirrors match.
pub fn check(root: &Path) -> Result<(), String> {
    validate(root)?;
    let canonical = tree(&root.join(CANONICAL))?;
    let mut problems = Vec::new();
    for mirror in MIRRORS {
        let copy = tree(&root.join(mirror))?;
        for (rel, bytes) in &canonical {
            match copy.get(rel) {
                None => problems.push(format!("{mirror}/{} is missing", slashed(rel))),
                Some(b) if b != bytes => {
                    problems.push(format!("{mirror}/{} differs", slashed(rel)))
                }
                _ => {}
            }
        }
        for rel in copy.keys().filter(|r| !canonical.contains_key(*r)) {
            problems.push(format!("{mirror}/{} is stale", slashed(rel)));
        }
    }
    if problems.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "skill mirrors drifted from {CANONICAL}/ — edit the canonical copy and run \
             `cargo xtask agents sync`:\n  {}",
            problems.join("\n  ")
        ))
    }
}

/// A relative path with `/` on every OS (the messages name repo paths).
fn slashed(rel: &Path) -> String {
    rel.components()
        .map(|c| c.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn skill(root: &Path, name: &str, body: &str) {
        let dir = root.join(CANONICAL).join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("SKILL.md"), body).unwrap();
    }

    fn good(name: &str) -> String {
        format!("---\nname: {name}\ndescription: Does {name} things.\n---\n\n# {name}\n")
    }

    #[test]
    fn sync_mirrors_the_canonical_tree_and_check_reports_drift() {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        skill(root, "alpha", &good("alpha"));
        skill(root, "beta", &good("beta"));
        std::fs::create_dir_all(root.join(CANONICAL).join("beta/refs")).unwrap();
        std::fs::write(root.join(CANONICAL).join("beta/refs/notes.md"), "n").unwrap();
        // A stale mirror file from an earlier skill.
        let mirror = root.join(MIRRORS[0]);
        std::fs::create_dir_all(mirror.join("gone")).unwrap();
        std::fs::write(mirror.join("gone/SKILL.md"), "old").unwrap();
        let err = check(root).unwrap_err();
        assert!(err.contains("alpha/SKILL.md is missing"), "{err}");
        assert!(err.contains("gone/SKILL.md is stale"), "{err}");

        assert_eq!(sync(root).unwrap(), 3 * MIRRORS.len());
        check(root).unwrap();
        assert!(!mirror.join("gone").exists(), "stale files are removed");
        assert_eq!(
            std::fs::read(mirror.join("beta/refs/notes.md")).unwrap(),
            b"n",
            "nested files are mirrored"
        );

        std::fs::write(mirror.join("alpha/SKILL.md"), "edited in the mirror").unwrap();
        let err = check(root).unwrap_err();
        assert!(err.contains("alpha/SKILL.md differs"), "{err}");
        assert!(err.contains("agents sync"), "the fix is named: {err}");
    }

    #[test]
    fn malformed_skills_are_rejected() {
        let cases = [
            (
                "no-front",
                "# no frontmatter\n",
                "missing `---` frontmatter",
            ),
            (
                "wrong-name",
                "---\nname: other\ndescription: x\n---\n",
                "must equal the folder name `wrong-name`",
            ),
            (
                "no-desc",
                "---\nname: no-desc\ndescription:\n---\n",
                "needs a description",
            ),
        ];
        for (name, body, msg) in cases {
            let tmp = tempfile::tempdir().unwrap();
            skill(tmp.path(), name, body);
            let err = validate(tmp.path()).unwrap_err();
            assert!(err.contains(msg), "{name}: {err}");
            assert!(sync(tmp.path()).is_err(), "{name}: sync refuses");
            assert!(check(tmp.path()).is_err(), "{name}: check refuses");
        }
        // A folder without SKILL.md.
        let tmp = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(tmp.path().join(CANONICAL).join("empty")).unwrap();
        let err = validate(tmp.path()).unwrap_err();
        assert!(err.contains("every skill needs a SKILL.md"), "{err}");
        // CRLF checkouts are fine.
        let tmp = tempfile::tempdir().unwrap();
        skill(tmp.path(), "crlf", &good("crlf").replace('\n', "\r\n"));
        validate(tmp.path()).unwrap();
        // No canonical folder at all.
        let tmp = tempfile::tempdir().unwrap();
        assert!(validate(tmp.path()).unwrap_err().contains("cannot read"));
    }
}
