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
                None => problems.push(format!("{mirror}/{} is missing", rel.display())),
                Some(b) if b != bytes => {
                    problems.push(format!("{mirror}/{} differs", rel.display()))
                }
                _ => {}
            }
        }
        for rel in copy.keys().filter(|r| !canonical.contains_key(*r)) {
            problems.push(format!("{mirror}/{} is stale", rel.display()));
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
