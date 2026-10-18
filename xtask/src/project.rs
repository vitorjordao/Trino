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
//! name = "my-game"        # output file stem (.z64 / .3dsx)
//! title = "My Game"         # ROM header (≤ 20 ASCII chars) + 3DS menu
//! description = "A console game"
//! author = "Studio"
//! platforms = ["pc", "n64", "3ds", "web"]  # optional; missing = pc, n64, 3ds
//!
//! [pc]
//! package = "my-game-pc"  # desktop host crate (optional; default <name>-pc)
//!
//! [n64]
//! package = "my-game-n64" # staticlib crate (cargo -p)
//! save = "eeprom16k"             # ed64romconfig save type
//!
//! [3ds]
//! package = "my-game-3ds" # bin crate (cargo -p)
//! bin = "my-game-3ds"     # the ELF's name
//! icon = "assets/platform/3ds/icon.png"   # 48x48 PNG (optional)
//!
//! [web]                   # optional, every field too (issue #47)
//! package = "my-game-web" # cdylib host crate (default <name>-web)
//! canvas_id = "trino-canvas"
//! page_title = "My Game"  # the page's <title> (default: title)
//!
//! [test]
//! n64_goldens = ["level"]         # TRINO_FB dumps compared in order
//! n3ds_goldens = ["level"]        # (tests/golden/<n64|3ds>/<name>.png)
//! n64_max_frame_us = 33333       # gate on TRINO_FRAME lines (30 fps)
//! watch = ["game", "n64", "assets"]       # dirs `watch n64|3ds` follows
//! ```
//!
//! `platforms` gates every platform command: an explicit `build n64` on a
//! project without `"n64"` fails naming the field, and the batch commands
//! (`build all`, `assets all`, `test all`) skip it. The `[n64]` / `[3ds]`
//! sections are only required for the platforms the project lists.

use std::path::{Path, PathBuf};

use serde::Deserialize;

/// A platform a project can target (`[project] platforms`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Target {
    Pc,
    N64,
    N3ds,
    /// A web browser (issue #47): `build|run|test web`.
    Web,
}

impl Target {
    /// Every platform, in the order batch commands visit them.
    pub const ALL: [Target; 4] = [Target::Pc, Target::N64, Target::N3ds, Target::Web];

    /// What a `trino.toml` without `platforms` targets: the three a project
    /// targeted before the Web existed (the Web needs a `web` host crate,
    /// so it is opted into by name).
    pub const DEFAULT: [Target; 3] = [Target::Pc, Target::N64, Target::N3ds];

    /// The spelling `trino.toml` and the command line use.
    pub fn key(self) -> &'static str {
        match self {
            Target::Pc => "pc",
            Target::N64 => "n64",
            Target::N3ds => "3ds",
            Target::Web => "web",
        }
    }

    /// `pc`, `n64`, `3ds`, `web` (and the command line's `n3ds` alias).
    pub fn parse(s: &str) -> Option<Target> {
        match s {
            "pc" => Some(Target::Pc),
            "n64" => Some(Target::N64),
            "3ds" | "n3ds" => Some(Target::N3ds),
            "web" => Some(Target::Web),
            _ => None,
        }
    }
}

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
    /// `[project] platforms`, deduplicated in [`Target::ALL`] order; every
    /// platform when the field is missing (and for the engine itself).
    pub platforms: Vec<Target>,
    /// `trino.toml` (None for the engine's own demo): named by the errors.
    pub manifest: Option<PathBuf>,
    /// Desktop host crate (`[pc] package`, default `<name>-pc`).
    pub pc_package: String,
    pub n64_package: String,
    pub n64_save: String,
    pub n3ds_package: String,
    pub n3ds_bin: String,
    pub n3ds_icon: Option<PathBuf>,
    /// The Web host crate (`[web] package`, default `<name>-web`), the
    /// page's canvas id and title (`[web] canvas_id`, `page_title`).
    pub web_package: String,
    pub web_canvas_id: String,
    pub web_title: String,
    pub test: TestConfig,
    pub watch: Vec<String>,
    /// Rust toolchain for the console builds (`cargo +<nightly>`):
    /// `$TRINO_NIGHTLY`, else `[toolchain] nightly`, else `nightly`.
    /// Pin it: N64 frame times move with code layout, so two nightlies
    /// give two different emulated timings for the same source.
    pub nightly: String,
}

/// `[toolchain]`: the nightly the console builds use.
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolchainSection {
    #[serde(default)]
    pub nightly: Option<String>,
}

/// `$TRINO_NIGHTLY` over `pinned` over the floating `nightly`.
fn nightly_from(env: Option<String>, pinned: Option<String>) -> String {
    env.filter(|s| !s.is_empty())
        .or(pinned)
        .unwrap_or_else(|| "nightly".into())
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
    #[serde(default)]
    pc: Option<PcSection>,
    #[serde(default)]
    n64: Option<N64Section>,
    #[serde(default, rename = "3ds")]
    n3ds: Option<N3dsSection>,
    #[serde(default)]
    web: Option<WebSection>,
    #[serde(default)]
    test: TestConfig,
    #[serde(default)]
    toolchain: ToolchainSection,
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
    /// Missing = every platform (the behavior before the field existed).
    #[serde(default)]
    platforms: Option<Vec<String>>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PcSection {
    package: String,
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

/// `[web]`: every field optional.
#[derive(Deserialize, Default)]
#[serde(deny_unknown_fields)]
struct WebSection {
    #[serde(default)]
    package: Option<String>,
    #[serde(default)]
    canvas_id: Option<String>,
    #[serde(default)]
    page_title: Option<String>,
}

/// The page's canvas id unless `[web] canvas_id` says otherwise.
pub const DEFAULT_CANVAS_ID: &str = "trino-canvas";

fn default_save() -> String {
    "eeprom16k".into()
}

/// `[project] platforms`: missing = all; otherwise known names, at least
/// one, returned deduplicated in [`Target::ALL`] order.
fn parse_platforms(list: Option<&[String]>) -> Result<Vec<Target>, String> {
    let Some(list) = list else {
        return Ok(Target::DEFAULT.to_vec());
    };
    let mut out = Vec::new();
    for name in list {
        // `n3ds` is a command-line alias, not a manifest spelling.
        let t = Target::parse(name)
            .filter(|_| name != "n3ds")
            .ok_or_else(|| {
                format!(
                    "[project] platforms: unknown platform `{name}` (use \"pc\", \"n64\", \"3ds\", \"web\")"
                )
            })?;
        if !out.contains(&t) {
            out.push(t);
        }
    }
    if out.is_empty() {
        return Err(
            "[project] platforms is empty: list at least one of \"pc\", \"n64\", \"3ds\" \
             (or remove the field to target all three)"
                .into(),
        );
    }
    out.sort();
    Ok(out)
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
            platforms: Target::ALL.to_vec(),
            manifest: None,
            pc_package: "trino-app-pc".into(),
            n64_package: "trino-app-n64".into(),
            n64_save: default_save(),
            n3ds_package: "trino-app-3ds".into(),
            n3ds_bin: "trino-3ds".into(),
            n3ds_icon: None,
            web_package: "trino-app-web".into(),
            web_canvas_id: DEFAULT_CANVAS_ID.into(),
            web_title: "Trino".into(),
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
            nightly: nightly_from(std::env::var("TRINO_NIGHTLY").ok(), None),
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
        let mut p = Self::parse(&text, root, engine_root())
            .map_err(|e| format!("{}: {e}", path.display()))?;
        p.manifest = Some(path);
        Ok(p)
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
        let platforms = parse_platforms(f.project.platforms.as_deref())?;
        let wants = |t: Target| platforms.contains(&t);
        if wants(Target::N64) && f.n64.is_none() {
            return Err(
                "[n64] is missing: add it (package = \"<crate>\"), or leave \"n64\" out of \
                 [project] platforms"
                    .into(),
            );
        }
        if wants(Target::N3ds) && f.n3ds.is_none() {
            return Err(
                "[3ds] is missing: add it (package, bin), or leave \"3ds\" out of \
                 [project] platforms"
                    .into(),
            );
        }
        let n64 = f.n64.unwrap_or(N64Section {
            package: String::new(),
            save: default_save(),
        });
        let n3ds = f.n3ds.unwrap_or(N3dsSection {
            package: String::new(),
            bin: String::new(),
            icon: None,
        });
        let web = f.web.unwrap_or_default();
        let web_package = web
            .package
            .unwrap_or_else(|| format!("{}-web", f.project.name));
        let web_canvas_id = web.canvas_id.unwrap_or_else(|| DEFAULT_CANVAS_ID.into());
        let web_title = web.page_title.unwrap_or_else(|| f.project.title.clone());
        let watch = f.test.watch.clone().unwrap_or_else(|| {
            ["game", "n64", "n3ds", "3ds", "assets"]
                .iter()
                .filter(|d| root.join(d).is_dir())
                .map(|d| d.to_string())
                .collect()
        });
        Ok(Project {
            n3ds_icon: n3ds.icon.map(|i| root.join(i)),
            is_engine: root == engine,
            engine,
            pc_package: f
                .pc
                .map(|pc| pc.package)
                .unwrap_or_else(|| format!("{}-pc", f.project.name)),
            name: f.project.name,
            title: f.project.title,
            description: f.project.description,
            author: f.project.author,
            platforms,
            manifest: None,
            n64_package: n64.package,
            n64_save: n64.save,
            n3ds_package: n3ds.package,
            n3ds_bin: n3ds.bin,
            web_package,
            web_canvas_id,
            web_title,
            test: f.test,
            watch,
            root,
            nightly: nightly_from(std::env::var("TRINO_NIGHTLY").ok(), f.toolchain.nightly),
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

    /// `Ok` when the project targets `t`; otherwise an error that names
    /// the `trino.toml` field to change.
    pub fn require(&self, t: Target) -> Result<(), String> {
        if self.platforms.contains(&t) {
            return Ok(());
        }
        let file = self
            .manifest
            .as_ref()
            .map_or_else(|| "trino.toml".to_string(), |m| m.display().to_string());
        let listed = self
            .platforms
            .iter()
            .map(|p| format!("\"{}\"", p.key()))
            .collect::<Vec<_>>()
            .join(", ");
        Err(format!(
            "project `{}` does not target {}: {file} has [project] platforms = [{listed}] \
             (add \"{}\" there to build it)",
            self.name,
            t.key(),
            t.key()
        ))
    }

    /// What a batch command (`build all`, `assets all`, `test all`) does on
    /// each platform: `(platform, runs)`, in [`Target::ALL`] order — the
    /// platforms the project leaves out are skipped.
    pub fn batch(&self) -> Vec<(Target, bool)> {
        Target::ALL
            .iter()
            .map(|t| (*t, self.platforms.contains(t)))
            .collect()
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
name = "my-game"
title = "My Game"
author = "Studio"

[n64]
package = "my-game-n64"

[3ds]
package = "my-game-3ds"
bin = "my-game-3ds"
icon = "assets/icon.png"

[test]
n64_goldens = ["level"]
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
            "target/mips-nintendo64-none/release/libmy_game_n64.a"
        );
        assert_eq!(p.rom(), PathBuf::from("/g/target/n64/my-game.z64"));
        assert_eq!(
            p.n3ds_elf(),
            PathBuf::from("/g/target/armv6k-nintendo-3ds/release/my-game-3ds.elf")
        );
        assert_eq!(p.n3ds_icon, Some(PathBuf::from("/g/assets/icon.png")));
        assert_eq!(p.test.n64_goldens, ["level"]);
        assert_eq!(p.test.n64_max_frame_us, Some(33333));
        assert_eq!(p.watch, ["game"]);
    }

    #[test]
    fn nightly_pin_env_over_file_over_floating() {
        let pinned = Some("nightly-2026-01-02".to_string());
        assert_eq!(nightly_from(None, None), "nightly");
        assert_eq!(nightly_from(None, pinned.clone()), "nightly-2026-01-02");
        assert_eq!(
            nightly_from(Some("nightly-2026-03-04".into()), pinned.clone()),
            "nightly-2026-03-04"
        );
        assert_eq!(
            nightly_from(Some(String::new()), pinned),
            "nightly-2026-01-02"
        );
        let with = format!("{SAMPLE}\n[toolchain]\nnightly = \"nightly-2026-05-06\"\n");
        let p = Project::parse(&with, "/g".into(), "/e".into()).unwrap();
        if std::env::var("TRINO_NIGHTLY").is_err() {
            assert_eq!(p.nightly, "nightly-2026-05-06");
        }
    }

    #[test]
    fn rejects_bad_names_and_titles() {
        let bad = SAMPLE.replace("my-game\"\ntitle", "my game\"\ntitle");
        assert!(Project::parse(&bad, "/g".into(), "/e".into()).is_err());
        let long = SAMPLE.replace("My Game", "A title far too long for the N64");
        assert!(Project::parse(&long, "/g".into(), "/e".into()).is_err());
        let accent = SAMPLE.replace("My Game", "My Game ç");
        assert!(Project::parse(&accent, "/g".into(), "/e".into()).is_err());
    }

    /// A manifest written before `platforms` existed parses as it always
    /// did and targets every platform.
    #[test]
    fn a_manifest_without_platforms_targets_everything() {
        let p = Project::parse(SAMPLE, "/g".into(), "/e".into()).unwrap();
        assert_eq!(p.platforms, Target::ALL);
        assert_eq!(p.pc_package, "my-game-pc");
        assert_eq!(p.n64_package, "my-game-n64");
        assert_eq!(p.n3ds_bin, "my-game-3ds");
        assert!(p.batch().iter().all(|(_, runs)| *runs));
        for t in Target::ALL {
            p.require(t).unwrap();
        }
    }

    fn with_platforms(list: &str) -> String {
        SAMPLE.replace(
            "author = \"Studio\"\n",
            &format!("author = \"Studio\"\nplatforms = {list}\n"),
        )
    }

    #[test]
    fn platforms_parse_dedupe_and_sort() {
        let p = Project::parse(
            &with_platforms(r#"["3ds", "pc", "3ds"]"#),
            "/g".into(),
            "/e".into(),
        )
        .unwrap();
        assert_eq!(p.platforms, [Target::Pc, Target::N3ds]);
        assert_eq!(
            p.batch(),
            [
                (Target::Pc, true),
                (Target::N64, false),
                (Target::N3ds, true)
            ]
        );
        let pc = format!(
            "{}\n[pc]\npackage = \"desk\"\n",
            with_platforms(r#"["pc"]"#)
        );
        let p = Project::parse(&pc, "/g".into(), "/e".into()).unwrap();
        assert_eq!(p.pc_package, "desk");
    }

    #[test]
    fn console_sections_are_only_required_for_listed_platforms() {
        // PC only: no [n64] and no [3ds] at all.
        let pc_only = "[project]\nname = \"my-game\"\ntitle = \"My Game\"\nplatforms = [\"pc\"]\n";
        let p = Project::parse(pc_only, "/g".into(), "/e".into()).unwrap();
        assert_eq!(p.platforms, [Target::Pc]);
        assert!(p.n64_package.is_empty() && p.n3ds_package.is_empty());
        // Listed but missing: the error says which section and the way out.
        let n64 = pc_only.replace(r#"["pc"]"#, r#"["pc", "n64"]"#);
        let e = Project::parse(&n64, "/g".into(), "/e".into()).unwrap_err();
        assert!(e.contains("[n64] is missing"), "{e}");
        assert!(e.contains("[project] platforms"), "{e}");
        let n3ds = pc_only.replace(r#"["pc"]"#, r#"["3ds"]"#);
        let e = Project::parse(&n3ds, "/g".into(), "/e".into()).unwrap_err();
        assert!(e.contains("[3ds] is missing"), "{e}");
        // Without the field every section is needed, as before.
        let e = Project::parse(
            &pc_only.replace("platforms = [\"pc\"]\n", ""),
            "/g".into(),
            "/e".into(),
        )
        .unwrap_err();
        assert!(e.contains("[n64] is missing"), "{e}");
    }

    #[test]
    fn bad_platform_lists_are_rejected() {
        for (list, needle) in [
            (r#"["pc", "wii"]"#, "unknown platform `wii`"),
            (r#"["n3ds"]"#, "unknown platform `n3ds`"),
            ("[]", "platforms is empty"),
        ] {
            let e = Project::parse(&with_platforms(list), "/g".into(), "/e".into()).unwrap_err();
            assert!(e.contains(needle), "{list}: {e}");
        }
        let e = Project::parse(&with_platforms("\"pc\""), "/g".into(), "/e".into()).unwrap_err();
        assert!(e.contains("platforms"), "{e}");
    }

    #[test]
    fn requiring_a_missing_platform_names_the_field_and_the_file() {
        let mut p = Project::parse(
            &with_platforms(r#"["pc", "3ds"]"#),
            "/g".into(),
            "/e".into(),
        )
        .unwrap();
        p.manifest = Some("/g/trino.toml".into());
        p.require(Target::Pc).unwrap();
        p.require(Target::N3ds).unwrap();
        let e = p.require(Target::N64).unwrap_err();
        assert!(e.contains("does not target n64"), "{e}");
        assert!(
            e.contains(r#"/g/trino.toml has [project] platforms = ["pc", "3ds"]"#),
            "{e}"
        );
        assert!(e.contains(r#"add "n64""#), "{e}");
    }

    #[test]
    fn targets_parse_their_command_line_spellings() {
        for t in Target::ALL {
            assert_eq!(Target::parse(t.key()), Some(t));
        }
        assert_eq!(Target::parse("n3ds"), Some(Target::N3ds));
        assert_eq!(Target::parse("xbox"), None);
    }

    #[test]
    fn engine_default_matches_the_in_repo_apps() {
        let p = Project::engine();
        assert!(p.is_engine);
        assert_eq!(p.platforms, Target::ALL);
        assert_eq!(
            p.n64_staticlib(),
            "target/mips-nintendo64-none/release/libtrino_app_n64.a"
        );
        assert!(p.rom().ends_with("target/n64/trino.z64"));
        assert!(p.n3ds_elf().ends_with("release/trino-3ds.elf"));
    }
}
