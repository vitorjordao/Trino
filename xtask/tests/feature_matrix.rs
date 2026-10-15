//! `docs/TESTING.md` maps every engine feature to the tests that exercise it.
//! This guard keeps the matrix honest in both directions:
//!
//! - every test reference (`` `path/to/file.rs::fn_name` ``) names a function
//!   that exists in that file and carries `#[test]`;
//! - every feature the code exposes mechanically has a row: each method of the
//!   `Renderer`, `Platform` and `Audio` traits, each TMDL section the mesh
//!   parser reads, each manifest asset kind and each top-level xtask command.
//!
//! A row's test cell either lists test references or starts with `n/a:` and a
//! reason (for commands that need Docker, an emulator or Blender).
//!
//! Fix a failure by adding the missing row (with a test), or by fixing the
//! reference after renaming or moving a test.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read(rel: &str) -> String {
    std::fs::read_to_string(root().join(rel)).unwrap_or_else(|e| panic!("{rel}: {e}"))
}

/// One matrix row: the backticked feature key and its test cell.
struct Row {
    line: usize,
    key: String,
    cell: String,
}

/// Rows of every two-column table whose first cell is a backticked key.
fn rows(matrix: &str) -> Vec<Row> {
    let mut out = Vec::new();
    for (i, line) in matrix.lines().enumerate() {
        let line = line.trim();
        if !line.starts_with('|') || line.starts_with("|---") || line.starts_with("| ---") {
            continue;
        }
        let cells: Vec<&str> = line.trim_matches('|').split('|').map(str::trim).collect();
        if cells.len() < 2 {
            continue;
        }
        let first = cells[0];
        if !(first.starts_with('`') && first.ends_with('`') && first.len() > 2) {
            continue; // a header row
        }
        out.push(Row {
            line: i + 1,
            key: first.trim_matches('`').to_string(),
            cell: cells[1..].join("|"),
        });
    }
    out
}

/// `` `file.rs::name` `` references in a cell.
fn refs(cell: &str) -> Vec<(String, String)> {
    cell.split('`')
        .skip(1)
        .step_by(2)
        .filter_map(|r| {
            let (file, func) = r.rsplit_once("::")?;
            file.ends_with(".rs")
                .then(|| (file.to_string(), func.to_string()))
        })
        .collect()
}

/// Does `source` define `fn name(` with `#[test]` among its attributes?
fn has_test_fn(source: &str, name: &str) -> bool {
    let lines: Vec<&str> = source.lines().collect();
    for (i, line) in lines.iter().enumerate() {
        let t = line.trim_start();
        let sig = t.strip_prefix("pub ").unwrap_or(t);
        if !(sig.starts_with(&format!("fn {name}(")) || sig.starts_with(&format!("fn {name}<"))) {
            continue;
        }
        // Walk up over attributes, doc comments and blank lines.
        for prev in lines[..i].iter().rev() {
            let p = prev.trim();
            if p == "#[test]" {
                return true;
            }
            if !(p.starts_with("#[") || p.starts_with("//") || p.is_empty()) {
                break;
            }
        }
    }
    false
}

/// `fn` names declared inside `pub trait <name> {` (up to its closing brace
/// at column 0).
fn trait_methods(rel: &str, name: &str) -> Vec<String> {
    let src = read(rel);
    let start = src
        .find(&format!("pub trait {name}"))
        .unwrap_or_else(|| panic!("{rel}: no `pub trait {name}`"));
    let body = &src[start..];
    let end = body.find("\n}").expect("trait end");
    body[..end]
        .lines()
        .filter_map(|l| {
            let l = l.trim_start();
            let rest = l.strip_prefix("fn ")?;
            Some(rest[..rest.find(['(', '<'])?].to_string())
        })
        .collect()
}

/// Section tags matched in `Mesh::from_tmdl` (`b"BNDS" ... =>`).
fn tmdl_sections() -> Vec<String> {
    let src = read("crates/core/src/render3d.rs");
    let start = src.find("pub fn from_tmdl").expect("from_tmdl");
    let end = start + src[start..].find("\n    pub fn ").expect("next fn");
    let mut tags = BTreeSet::new();
    for line in src[start..end].lines() {
        let l = line.trim_start();
        if let Some(rest) = l.strip_prefix("b\"")
            && rest.len() > 5
            && rest.as_bytes()[4] == b'"'
            && l.contains("=>")
        {
            tags.insert(rest[..4].to_string());
        }
    }
    tags.into_iter().collect()
}

/// `BTreeMap` fields of the asset manifest (the asset kinds).
fn manifest_kinds() -> Vec<String> {
    let src = read("crates/asset-pipeline/src/manifest.rs");
    let start = src.find("pub struct Manifest {").expect("Manifest");
    let end = start + src[start..].find("\n}").unwrap();
    src[start..end]
        .lines()
        .filter_map(|l| {
            let rest = l.trim().strip_prefix("pub ")?;
            let (field, ty) = rest.split_once(':')?;
            ty.contains("BTreeMap").then(|| field.trim().to_string())
        })
        .collect()
}

/// Top-level xtask invocations: `(Some("cmd"), Some("arg"))` or
/// `(Some("cmd"), _)` arms of `main`.
fn xtask_commands() -> Vec<String> {
    let src = read("xtask/src/main.rs");
    let mut out = BTreeSet::new();
    for line in src.lines() {
        let l = line.trim_start();
        let Some(rest) = l.strip_prefix("(Some(\"") else {
            continue;
        };
        let Some(cmd_end) = rest.find('"') else {
            continue;
        };
        let cmd = &rest[..cmd_end];
        let after = &rest[cmd_end..];
        if !after.contains("=>") {
            continue;
        }
        // `Some("pc")`, `Some("3ds" | "n3ds")`, `None | Some("pc")`, `_`.
        let arg = after
            .split_once(", ")
            .map(|(_, a)| a)
            .and_then(|a| a.split_once("Some(\""))
            .and_then(|(_, a)| a.split_once('"'))
            .map(|(a, _)| a);
        out.insert(match arg {
            Some(a) => format!("xtask {cmd} {a}"),
            None => format!("xtask {cmd}"),
        });
    }
    out.into_iter().collect()
}

fn matrix() -> (String, Vec<Row>) {
    let text = read("docs/TESTING.md");
    let rows = rows(&text);
    assert!(!rows.is_empty(), "docs/TESTING.md has no matrix rows");
    (text, rows)
}

#[test]
fn every_matrix_reference_is_an_existing_test() {
    let (_, rows) = matrix();
    let mut sources: BTreeMap<String, String> = BTreeMap::new();
    let mut problems = Vec::new();
    for row in &rows {
        let r = refs(&row.cell);
        let exempt = row.cell.trim_start().starts_with("n/a:");
        if r.is_empty() && !exempt {
            problems.push(format!(
                "line {}: `{}` lists no test (add one, or `n/a: <reason>`)",
                row.line, row.key
            ));
        }
        for (file, func) in r {
            let path = root().join(&file);
            let src = sources
                .entry(file.clone())
                .or_insert_with(|| std::fs::read_to_string(Path::new(&path)).unwrap_or_default());
            if src.is_empty() {
                problems.push(format!("line {}: {file} does not exist", row.line));
            } else if !has_test_fn(src, &func) {
                problems.push(format!(
                    "line {}: {file} has no #[test] fn `{func}`",
                    row.line
                ));
            }
        }
    }
    assert!(
        problems.is_empty(),
        "docs/TESTING.md is out of date:\n  {}",
        problems.join("\n  ")
    );
}

#[test]
fn every_engine_feature_has_a_matrix_row() {
    let (_, rows) = matrix();
    let keys: BTreeSet<&str> = rows.iter().map(|r| r.key.as_str()).collect();
    let mut wanted = Vec::new();
    for (file, name) in [
        ("crates/core/src/render.rs", "Renderer"),
        ("crates/core/src/platform.rs", "Platform"),
        ("crates/core/src/audio.rs", "Audio"),
    ] {
        let methods = trait_methods(file, name);
        assert!(methods.len() >= 4, "{name}: parsed only {methods:?}");
        wanted.extend(methods.into_iter().map(|m| format!("{name}::{m}")));
    }
    let tags = tmdl_sections();
    assert!(
        tags.contains(&"SKEL".to_string()),
        "TMDL tags parsed: {tags:?}"
    );
    wanted.extend(tags.into_iter().map(|t| format!("TMDL {t}")));
    let kinds = manifest_kinds();
    assert!(kinds.contains(&"sprites".to_string()), "kinds: {kinds:?}");
    wanted.extend(kinds.into_iter().map(|k| format!("asset {k}")));
    let commands = xtask_commands();
    assert!(
        commands.contains(&"xtask scene".to_string()),
        "commands: {commands:?}"
    );
    wanted.extend(commands);

    let missing: Vec<&String> = wanted
        .iter()
        .filter(|w| !keys.contains(w.as_str()))
        .collect();
    assert!(
        missing.is_empty(),
        "features without a row in docs/TESTING.md: {missing:?}"
    );
}

#[test]
fn the_guard_parses_its_own_inputs() {
    let table = "| Feature | Tests |\n|---|---|\n| `A::b` | `x/y.rs::t1`, `x/y.rs::t2` |\n| `C` | n/a: needs Docker |\n";
    let r = rows(table);
    assert_eq!(r.len(), 2);
    assert_eq!(r[0].key, "A::b");
    assert_eq!(
        refs(&r[0].cell),
        vec![
            ("x/y.rs".to_string(), "t1".to_string()),
            ("x/y.rs".to_string(), "t2".to_string())
        ]
    );
    assert!(refs(&r[1].cell).is_empty());
    let src = "#[test]\n#[ignore = \"slow\"]\n/// doc\nfn ok() {}\nfn helper() {}\n";
    assert!(has_test_fn(src, "ok"));
    assert!(!has_test_fn(src, "helper"));
    assert!(!has_test_fn(src, "absent"));
}
