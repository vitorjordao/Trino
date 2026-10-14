//! Which game a console build packages: the engine's own demo (castle64,
//! `apps/n64` + `apps/3ds`), or an **external project** described by a
//! `trino.toml` at its root (BACKLOG P1 #3).
//!
//! An external project is a cargo workspace with its own `assets/`, a game
//! crate, and one thin host crate per console built on the platform crates'
//! entry macros (`trino_platform_n64::n64_main!`,
//! `trino_platform_3ds::n3ds_main!`). The engine still owns everything
//! native: the shims, the target spec, the Docker image, the emulators.
//! Outputs land in the project's own `target/{n64,3ds}/`.
//!
//! Selection, first match wins: `--project <dir>`, `$TRINO_PROJECT`, a
//! `trino.toml` in the current directory (so `cargo run --manifest-path
//! engine/Cargo.toml -p xtask -- build n64` from a project root just
//! works), else the engine itself.
//!
//! ```toml
//! [project]
//! name = "plan-and-party"        # output file stem (.z64 / .3dsx)
//! title = "Plan & Party"         # ROM header (≤ 20 ASCII chars) + 3DS menu
//! description = "Party venue tycoon"
//! author = "Studio"
//!
//! [n64]
//! package = "plan-and-party-n64" # staticlib crate (cargo -p)
//! save = "eeprom16k"             # ed64romconfig save type
//!
//! [3ds]
//! package = "plan-and-party-3ds" # bin crate (cargo -p)
//! bin = "plan-and-party-3ds"     # the ELF's name
//! icon = "assets/platform/3ds/icon.png"   # 48x48 PNG (optional)
//!
//! [test]
//! n64_goldens = ["hall"]         # TRINO_FB dumps compared in order
//! n3ds_goldens = ["hall"]        # (tests/golden/<n64|3ds>/<name>.png)
//! n64_max_frame_us = 33333       # gate on TRINO_FRAME lines (30 fps)
//! watch = ["game", "n64", "assets"]       # dirs `watch n64|3ds` follows
//! ```

use std::path::{Path, PathBuf};

use serde::Deserialize;

#[derive(Clone, Debug)]
pub struct Project {
    /// Engine checkout: shims, target spec, docker/, emulators.
    pub engine: PathBuf,
    /// Project root: `assets/`, `target/`, `tests/golden/`.
    pub root: PathBuf,
    /// `true` for the engine's own demo: castle64's goldens and perf gates
    /// apply.
    pub is_engine: bool,
    pub name: String,
    pub title: String,
    pub description: String,
    pub author: String,
    pub n64_package: String,
    pub n64_save: String,
    pub n3ds_package: String,
    pub n3ds_bin: String,
    pub n3ds_icon: Option<PathBuf>,
    pub test: TestConfig,
    pub watch: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct TestConfig {
    #[serde(default)]
    pub n64_goldens: Vec<String>,
    #[serde(default)]
    pub n3ds_goldens: Vec<String>,
    #[serde(default)]
    pub n64_max_frame_us: Option<u32>,
    /// Mean per-channel distance allowed by the 3DS golden (Azahar's pixels
    /// depend on the host OpenGL). Default 3.0.
    #[serde(default)]
    pub n3ds_golden_mean: Option<f32>,
    #[serde(default)]
    pub watch: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct File {
    project: ProjectSection,
    n64: N64Section,
    #[serde(rename = "3ds")]
    n3ds: N3dsSection,
    #[serde(default)]
    test: TestConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct ProjectSection {
    name: String,
    title: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    author: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct N64Section {
    package: String,
    #[serde(default = "default_save")]
    save: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct N3dsSection {
    package: String,
    bin: String,
    #[serde(default)]
    icon: Option<String>,
}

fn default_save() -> String {
    "eeprom16k".into()
}

/// The engine checkout xtask was built from.
pub fn engine_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("engine root");
    strip_verbatim(root)
}

/// canonicalize() yields \\?\C:\... on Windows; mingw-built devkitPro tools
/// and docker path conversion reject the verbatim prefix.
fn strip_verbatim(p: PathBuf) -> PathBuf {
    let s = p.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => p,
    }
}

impl Project {
    /// The engine's own demo: castle64 through `apps/n64` and `apps/3ds`.
    pub fn engine() -> Self {
        let engine = engine_root();
        Project {
            root: engine.clone(),
            engine,
            is_engine: true,
            name: "trino".into(),
            title: "Trino".into(),
            description: "Trino castle64".into(),
            author: "Trino".into(),
            n64_package: "trino-app-n64".into(),
            n64_save: default_save(),
            n3ds_package: "trino-app-3ds".into(),
            n3ds_bin: "trino-3ds".into(),
            n3ds_icon: None,
            test: TestConfig::default(),
            watch: [
                "examples",
                "assets",
                "apps",
                "crates/platform-n64",
                "crates/platform-3ds",
            ]
            .map(String::from)
            .to_vec(),
        }
    }

    /// Parse `<dir>/trino.toml`.
    pub fn load(dir: &Path) -> Result<Self, String> {
        let root = strip_verbatim(
            dir.canonicalize()
                .map_err(|e| format!("project {}: {e}", dir.display()))?,
        );
        let path = root.join("trino.toml");
        let text =
            std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        Self::parse(&text, root, engine_root()).map_err(|e| format!("{}: {e}", path.display()))
    }

    fn parse(text: &str, root: PathBuf, engine: PathBuf) -> Result<Self, String> {
        let f: File = toml::from_str(text).map_err(|e| e.to_string())?;
        if f.project.name.is_empty()
            || !f
                .project
                .name
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        {
            return Err(format!(
                "project.name `{}`: use [A-Za-z0-9_-] (it names the output files)",
                f.project.name
            ));
        }
        if !f.project.title.is_ascii() || f.project.title.len() > 20 {
            return Err(format!(
                "project.title `{}`: the N64 ROM header holds ≤ 20 ASCII characters",
                f.project.title
            ));
        }
        let watch = f.test.watch.clone().unwrap_or_else(|| {
            ["game", "n64", "n3ds", "3ds", "assets"]
                .iter()
                .filter(|d| root.join(d).is_dir())
                .map(|d| d.to_string())
                .collect()
        });
        Ok(Project {
            n3ds_icon: f.n3ds.icon.map(|i| root.join(i)),
            is_engine: root == engine,
            engine,
            name: f.project.name,
            title: f.project.title,
            description: f.project.description,
            author: f.project.author,
            n64_package: f.n64.package,
            n64_save: f.n64.save,
            n3ds_package: f.n3ds.package,
            n3ds_bin: f.n3ds.bin,
            test: f.test,
            watch,
            root,
        })
    }

    /// `--project <dir>` (removed from `args`), `$TRINO_PROJECT`, a
    /// `trino.toml` in the current directory, else the engine.
    pub fn select(args: &mut Vec<String>) -> Result<Self, String> {
        if let Some(i) = args.iter().position(|a| a == "--project") {
            if i + 1 >= args.len() {
                return Err("--project needs a directory".into());
            }
            let dir = PathBuf::from(args.remove(i + 1));
            args.remove(i);
            return Self::load(&dir);
        }
        if let Ok(dir) = std::env::var("TRINO_PROJECT")
            && !dir.is_empty()
        {
            return Self::load(Path::new(&dir));
        }
        if let Ok(cwd) = std::env::current_dir()
            && cwd.join("trino.toml").is_file()
        {
            return Self::load(&cwd);
        }
        Ok(Self::engine())
    }

    /// Output ROM / app: `target/n64/<name>.z64`, `target/3ds/<name>.3dsx`.
    pub fn rom(&self) -> PathBuf {
        self.root.join(format!("target/n64/{}.z64", self.name))
    }

    pub fn app_3dsx(&self) -> PathBuf {
        self.root.join(format!("target/3ds/{}.3dsx", self.name))
    }

    /// The staticlib cargo writes for the N64 host crate, relative to the
    /// project root (the in-container working directory).
    pub fn n64_staticlib(&self) -> String {
        format!(
            "target/mips-nintendo64-none/release/lib{}.a",
            self.n64_package.replace('-', "_")
        )
    }

    pub fn n3ds_elf(&self) -> PathBuf {
        self.root.join(format!(
            "target/armv6k-nintendo-3ds/release/{}.elf",
            self.n3ds_bin
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SAMPLE: &str = r#"
[project]
name = "plan-and-party"
title = "Plan & Party"
author = "Studio"

[n64]
package = "plan-and-party-n64"

[3ds]
package = "plan-and-party-3ds"
bin = "pnp-3ds"
icon = "assets/icon.png"

[test]
n64_goldens = ["hall"]
n64_max_frame_us = 33333
watch = ["game"]
"#;

    #[test]
    fn parses_a_project() {
        let p = Project::parse(SAMPLE, "/g".into(), "/e".into()).unwrap();
        assert!(!p.is_engine);
        assert_eq!(p.n64_save, "eeprom16k");
        assert_eq!(
            p.n64_staticlib(),
            "target/mips-nintendo64-none/release/libplan_and_party_n64.a"
        );
        assert_eq!(p.rom(), PathBuf::from("/g/target/n64/plan-and-party.z64"));
        assert_eq!(
            p.n3ds_elf(),
            PathBuf::from("/g/target/armv6k-nintendo-3ds/release/pnp-3ds.elf")
        );
        assert_eq!(p.n3ds_icon, Some(PathBuf::from("/g/assets/icon.png")));
        assert_eq!(p.test.n64_goldens, ["hall"]);
        assert_eq!(p.test.n64_max_frame_us, Some(33333));
        assert_eq!(p.watch, ["game"]);
    }

    #[test]
    fn rejects_bad_names_and_titles() {
        let bad = SAMPLE.replace("plan-and-party\"\ntitle", "plan and party\"\ntitle");
        assert!(Project::parse(&bad, "/g".into(), "/e".into()).is_err());
        let long = SAMPLE.replace("Plan & Party", "A title far too long for the N64");
        assert!(Project::parse(&long, "/g".into(), "/e".into()).is_err());
        let accent = SAMPLE.replace("Plan & Party", "Plan & Festa ç");
        assert!(Project::parse(&accent, "/g".into(), "/e".into()).is_err());
    }

    #[test]
    fn engine_default_matches_the_in_repo_apps() {
        let p = Project::engine();
        assert!(p.is_engine);
        assert_eq!(
            p.n64_staticlib(),
            "target/mips-nintendo64-none/release/libtrino_app_n64.a"
        );
        assert!(p.rom().ends_with("target/n64/trino.z64"));
        assert!(p.n3ds_elf().ends_with("release/trino-3ds.elf"));
    }
}
