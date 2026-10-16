//! `cargo xtask new <name> --standalone <dir>`: a game project **outside**
//! this repository. The rendering lives in the `trino-scaffold` crate
//! (shared with the editor's project launcher); this module parses the
//! command line and runs the interactive `cargo xtask new` (no name).
//!
//! - `--git <url> [--rev <rev> | --branch <b>]`: engine crates as git
//!   dependencies instead of path dependencies on this checkout.
//! - `--template blank|castle64|showcase`: the game crate starts from the
//!   blank template (default) or a copy of an engine demo.

use std::io::{BufRead, Write};
use std::path::{Path, PathBuf};

pub use trino_scaffold::{EngineSource, Names, Template, absolute, scaffold};

/// A parsed `--standalone` invocation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Standalone {
    pub dir: PathBuf,
    pub source: EngineSource,
    pub template: Template,
}

/// The `--template` values, for messages.
fn template_keys() -> String {
    Template::ALL
        .iter()
        .map(|t| t.key())
        .collect::<Vec<_>>()
        .join("|")
}

/// Parse `--standalone <dir> [--git <url> [--rev <r> | --branch <b>]]
/// [--template <t>]` (everything after the project name). `Ok(None)` = not
/// standalone.
pub fn parse_args(args: &[&str], engine_root: &Path) -> Result<Option<Standalone>, String> {
    let mut dir = None;
    let mut git = None;
    let mut pin = None;
    let mut template = None;
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
            "--template" => {
                let t = value("--template")?;
                template =
                    Some(Template::parse(&t).ok_or_else(|| {
                        format!("unknown template `{t}` (use {})", template_keys())
                    })?);
            }
            other => return Err(format!("unknown option `{other}`")),
        }
    }
    let Some(dir) = dir else {
        return if git.is_some() || pin.is_some() {
            Err("--git/--rev/--branch need --standalone <dir>".into())
        } else if template.is_some() {
            Err("--template needs --standalone <dir>".into())
        } else {
            Ok(None)
        };
    };
    let source = match git {
        Some(url) => EngineSource::Git { url, pin },
        None if pin.is_some() => return Err("--rev/--branch need --git <url>".into()),
        None => EngineSource::Path(absolute(engine_root)?),
    };
    Ok(Some(Standalone {
        dir,
        source,
        template: template.unwrap_or_default(),
    }))
}

/// One answer: `None` on end of input.
fn ask(input: &mut dyn BufRead, out: &mut dyn Write, question: &str) -> Option<String> {
    let _ = write!(out, "{question}");
    let _ = out.flush();
    let mut line = String::new();
    match input.read_line(&mut line) {
        Ok(0) | Err(_) => None,
        Ok(_) => Some(line.trim().to_string()),
    }
}

/// Interactive `cargo xtask new` (no name): ask for the name, the folder
/// and the template; a wrong answer is explained and asked again. `None`
/// when the input ends before the questions are answered (a closed or
/// empty stdin: the caller prints the usage, as before prompts existed).
/// The folder defaults to `<engine>/../<name>`, next to the engine
/// checkout rather than inside it.
pub fn prompt(
    input: &mut dyn BufRead,
    out: &mut dyn Write,
    engine_root: &Path,
) -> Option<(Names, Standalone)> {
    let names = loop {
        let name = ask(input, out, "Project name (kebab-case, e.g. my-game): ")?;
        match Names::new(&name) {
            Ok(n) => break n,
            Err(e) => {
                let _ = writeln!(out, "  {e}");
            }
        }
    };
    let parent = absolute(&engine_root.join("..")).unwrap_or_else(|_| engine_root.join(".."));
    let default_dir = parent.join(&names.kebab);
    let dir = loop {
        let answer = ask(input, out, &format!("Folder [{}]: ", default_dir.display()))?;
        let dir = if answer.is_empty() {
            default_dir.clone()
        } else {
            PathBuf::from(answer)
        };
        if std::fs::read_dir(&dir).is_ok_and(|mut d| d.next().is_some()) {
            let _ = writeln!(out, "  {} exists and is not empty", dir.display());
            continue;
        }
        break dir;
    };
    let _ = writeln!(out, "Templates:");
    for t in Template::ALL {
        let _ = writeln!(out, "  {:9} {}", t.key(), t.about());
    }
    let template = loop {
        let answer = ask(
            input,
            out,
            &format!("Template ({}) [blank]: ", template_keys()),
        )?;
        if answer.is_empty() {
            break Template::Blank;
        }
        match Template::parse(&answer) {
            Some(t) => break t,
            None => {
                let _ = writeln!(out, "  unknown template `{answer}`");
            }
        }
    };
    let source = EngineSource::Path(absolute(engine_root).ok()?);
    Some((
        names,
        Standalone {
            dir,
            source,
            template,
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
    }

    #[test]
    fn parses_sources() {
        let root = engine();
        assert_eq!(parse_args(&[], &root).unwrap(), None);
        let s = parse_args(&["--standalone", "/tmp/g"], &root)
            .unwrap()
            .unwrap();
        assert_eq!(s.dir, PathBuf::from("/tmp/g"));
        assert!(matches!(s.source, EngineSource::Path(_)));
        assert_eq!(s.template, Template::Blank);
        let s = parse_args(
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
            s.source.dependency_lines().lines().next().unwrap(),
            "trino-core = { git = \"https://x/t.git\", rev = \"abc\" }"
        );
        assert!(parse_args(&["--git", "u"], &root).is_err());
        assert!(parse_args(&["--standalone", "g", "--rev", "r"], &root).is_err());
        assert!(parse_args(&["--bogus"], &root).is_err());
        let abs = absolute(Path::new("a/b/../c")).unwrap();
        assert!(abs.is_absolute() && abs.ends_with("a/c"), "{abs:?}");
        assert!(!abs.to_string_lossy().contains(".."));
    }

    #[test]
    fn parses_templates() {
        let root = engine();
        let s = parse_args(&["--standalone", "g", "--template", "castle64"], &root)
            .unwrap()
            .unwrap();
        assert_eq!(s.template, Template::Demo(trino_scaffold::Demo::Castle64));
        let e = parse_args(&["--standalone", "g", "--template", "doom"], &root).unwrap_err();
        assert!(e.contains("unknown template `doom`"), "{e}");
        assert!(e.contains("blank|castle64|showcase"), "{e}");
        let e = parse_args(&["--template", "showcase"], &root).unwrap_err();
        assert!(e.contains("--template needs --standalone"), "{e}");
        assert!(parse_args(&["--standalone", "g", "--template"], &root).is_err());
    }

    /// The prompts re-ask after a wrong answer, default the folder next to
    /// the engine and the template to blank, and give up on end of input.
    #[test]
    fn prompts_ask_again_and_default() {
        let root = engine();
        let mut out = Vec::new();
        let answers = "Bad Name\nmy-game\n\nshooter\ncastle64\n";
        let (names, s) = prompt(&mut answers.as_bytes(), &mut out, &root).unwrap();
        let text = String::from_utf8(out).unwrap();
        assert_eq!(names.kebab, "my-game");
        assert!(text.contains("not a valid crate name"), "{text}");
        assert!(text.contains("unknown template `shooter`"), "{text}");
        assert_eq!(s.template, Template::Demo(trino_scaffold::Demo::Castle64));
        assert_eq!(
            s.dir,
            absolute(&root.join("..")).unwrap().join("my-game"),
            "next to the engine"
        );
        assert!(matches!(s.source, EngineSource::Path(_)));

        let (_, s) = prompt(
            &mut "my-game\nsome/dir\n\n".as_bytes(),
            &mut Vec::new(),
            &root,
        )
        .unwrap();
        assert_eq!(s.dir, PathBuf::from("some/dir"));
        assert_eq!(s.template, Template::Blank);

        // A non-empty folder is refused (this checkout's own root).
        let busy = format!("my-game\n{}\n", root.display());
        let mut out = Vec::new();
        assert!(prompt(&mut busy.as_bytes(), &mut out, &root).is_none());
        assert!(String::from_utf8(out).unwrap().contains("is not empty"));

        for cut in ["", "my-game\n", "my-game\nd\n"] {
            assert!(
                prompt(&mut cut.as_bytes(), &mut Vec::new(), &root).is_none(),
                "{cut:?}"
            );
        }
    }

    /// A scaffolded project (blank and from a demo) is a valid console
    /// project for the xtask.
    #[test]
    fn scaffolded_trino_toml_loads_as_a_project() {
        let root = absolute(&engine()).unwrap();
        let base = root.join("target/test-projects");
        std::fs::create_dir_all(&base).unwrap();
        let tmp = tempfile::tempdir_in(base).unwrap();
        let names = Names::new("my-game").unwrap();
        for t in [
            Template::Blank,
            Template::Demo(trino_scaffold::Demo::Showcase),
        ] {
            let dest = tmp.path().join(t.key());
            scaffold(&root, &dest, &names, &EngineSource::Path(root.clone()), t).unwrap();
            let p = crate::project::Project::load(&dest).unwrap();
            assert_eq!(p.name, "my-game");
            assert_eq!(p.pc_package, "my-game-pc");
            assert_eq!(p.platforms, crate::project::Target::ALL);
        }
    }
}
