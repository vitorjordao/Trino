//! `cargo xtask new <name> --standalone <dir>`: a game project **outside**
//! this repository. Renders `templates/standalone/` (workspace, PC host,
//! asset manifest, AGENTS.md) plus the shared game template
//! (`templates/new-game/src` + `Cargo.toml.tmpl`) into `<dir>`, with the engine
//! crates as path dependencies on this checkout (default) or as git
//! dependencies (`--git <url> [--rev <rev> | --branch <b>]`).
//!
//! The engine's `Cargo.lock` is copied in, so a fresh project resolves the
//! exact dependency versions the engine is tested with.

use std::path::{Path, PathBuf};

/// Where the engine crates come from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum EngineSource {
    /// A local checkout (absolute path to the repository root).
    Path(PathBuf),
    Git {
        url: String,
        /// `rev = ".."` or `branch = ".."` (None = default branch).
        pin: Option<(String, String)>,
    },
}

/// Engine crates a standalone project uses: `(package, path in the repo)`.
const ENGINE_CRATES: &[(&str, &str)] = &[
    ("trino-core", "crates/core"),
    ("trino-game-api", "crates/game-api"),
    ("trino-runner-pc", "crates/runner-pc"),
];

impl EngineSource {
    /// `[workspace.dependencies]` lines for the engine crates.
    pub fn dependency_lines(&self) -> String {
        ENGINE_CRATES
            .iter()
            .map(|(name, rel)| match self {
                EngineSource::Path(root) => {
                    let p = root.join(rel);
                    format!(
                        "{name} = {{ path = \"{}\" }}",
                        p.display().to_string().replace('\\', "/")
                    )
                }
                EngineSource::Git { url, pin } => match pin {
                    Some((kind, value)) => {
                        format!("{name} = {{ git = \"{url}\", {kind} = \"{value}\" }}")
                    }
                    None => format!("{name} = {{ git = \"{url}\" }}"),
                },
            })
            .collect::<Vec<_>>()
            .join("\n")
    }
}

/// Parse `--standalone <dir> [--git <url> [--rev <r> | --branch <b>]]`
/// (everything after the project name). `Ok(None)` = not standalone.
pub fn parse_args(
    args: &[&str],
    engine_root: &Path,
) -> Result<Option<(PathBuf, EngineSource)>, String> {
    let mut dir = None;
    let mut git = None;
    let mut pin = None;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut value = |flag: &str| {
            it.next()
                .map(|v| v.to_string())
                .ok_or(format!("{flag} needs a value"))
        };
        match *a {
            "--standalone" => dir = Some(PathBuf::from(value("--standalone")?)),
            "--git" => git = Some(value("--git")?),
            "--rev" => pin = Some(("rev".to_string(), value("--rev")?)),
            "--branch" => pin = Some(("branch".to_string(), value("--branch")?)),
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    let Some(dir) = dir else {
        return if git.is_some() || pin.is_some() {
            Err("--git/--rev/--branch need --standalone <dir>".into())
        } else {
            Ok(None)
        };
    };
    let source = match git {
        Some(url) => EngineSource::Git { url, pin },
        None if pin.is_some() => return Err("--rev/--branch need --git <url>".into()),
        None => EngineSource::Path(
            engine_root
                .canonicalize()
                .map_err(|e| format!("{}: {e}", engine_root.display()))?,
        ),
    };
    Ok(Some((dir, source)))
}

/// Render the standalone project into `dest` (must not exist or be empty).
pub fn scaffold(
    engine_root: &Path,
    dest: &Path,
    names: &crate::Names,
    source: &EngineSource,
) -> Result<(), String> {
    if dest.exists()
        && std::fs::read_dir(dest)
            .map_err(|e| e.to_string())?
            .next()
            .is_some()
    {
        return Err(format!("{} exists and is not empty", dest.display()));
    }
    let deps = source.dependency_lines();
    let render = |text: &str| names.render(text).replace("{{trino_deps}}", &deps);
    let templates = engine_root.join("templates");
    copy_rendered(&templates.join("standalone"), dest, &render)?;
    // The game crate is the same template `xtask new` uses in-repo.
    let game = dest.join("game");
    copy_rendered(&templates.join("new-game/src"), &game.join("src"), &render)?;
    let manifest = std::fs::read_to_string(templates.join("new-game/Cargo.toml.tmpl"))
        .map_err(|e| e.to_string())?;
    write(&game.join("Cargo.toml"), &render(&manifest))?;
    // Dotfiles are stored without the dot (they would apply to the engine
    // repo itself otherwise).
    std::fs::rename(dest.join("gitignore"), dest.join(".gitignore")).map_err(|e| e.to_string())?;
    // Starter assets: the engine's own hero sprite and beep.
    for rel in ["sprites/hero.png", "sounds/beep.wav"] {
        let to = dest.join("assets/shared").join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(engine_root.join("assets/shared").join(rel), &to)
            .map_err(|e| format!("{rel}: {e}"))?;
    }
    // Same dependency versions the engine is tested with.
    let lock = engine_root.join("Cargo.lock");
    if lock.exists() {
        std::fs::copy(&lock, dest.join("Cargo.lock")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

fn copy_rendered(from: &Path, to: &Path, render: &dyn Fn(&str) -> String) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| e.to_string())?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src = entry.path();
        let dst = to.join(crate::template_target_name(&entry.file_name()));
        if src.is_dir() {
            copy_rendered(&src, &dst, render)?;
        } else {
            let text =
                std::fs::read_to_string(&src).map_err(|e| format!("{}: {e}", src.display()))?;
            write(&dst, &render(&text))?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sources() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        assert_eq!(parse_args(&[], &root).unwrap(), None);
        let (dir, src) = parse_args(&["--standalone", "/tmp/g"], &root)
            .unwrap()
            .unwrap();
        assert_eq!(dir, PathBuf::from("/tmp/g"));
        assert!(matches!(src, EngineSource::Path(_)));
        let (_, src) = parse_args(
            &[
                "--standalone",
                "g",
                "--git",
                "https://x/t.git",
                "--rev",
                "abc",
            ],
            &root,
        )
        .unwrap()
        .unwrap();
        assert_eq!(
            src.dependency_lines().lines().next().unwrap(),
            "trino-core = { git = \"https://x/t.git\", rev = \"abc\" }"
        );
        assert!(parse_args(&["--git", "u"], &root).is_err());
        assert!(parse_args(&["--standalone", "g", "--rev", "r"], &root).is_err());
        assert!(parse_args(&["--bogus"], &root).is_err());
    }

    #[test]
    fn renders_a_complete_project() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("..");
        let tmp = tempfile::tempdir().unwrap();
        let dest = tmp.path().join("my-game");
        let names = crate::Names::new("my-game").unwrap();
        let src = EngineSource::Path(root.canonicalize().unwrap());
        scaffold(&root, &dest, &names, &src).unwrap();
        for f in [
            "Cargo.toml",
            "Cargo.lock",
            ".gitignore",
            "AGENTS.md",
            "game/Cargo.toml",
            "game/src/lib.rs",
            "pc/src/main.rs",
            "assets/manifest.toml",
            "assets/shared/sprites/hero.png",
        ] {
            assert!(dest.join(f).exists(), "{f}");
        }
        let ws = std::fs::read_to_string(dest.join("Cargo.toml")).unwrap();
        assert!(ws.contains("my-game = { path = \"game\" }"));
        assert!(ws.contains("trino-runner-pc = { path = \""), "{ws}");
        let main = std::fs::read_to_string(dest.join("pc/src/main.rs")).unwrap();
        assert!(main.contains("my_game::MyGameGame::new"), "{main}");
        for f in [
            "Cargo.toml",
            "pc/Cargo.toml",
            "game/src/lib.rs",
            "AGENTS.md",
        ] {
            let t = std::fs::read_to_string(dest.join(f)).unwrap();
            assert!(!t.contains("{{"), "{f} has leftover placeholders");
        }
        assert!(scaffold(&root, &dest, &names, &src).is_err(), "not empty");
    }
}
