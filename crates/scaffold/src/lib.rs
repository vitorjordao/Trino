//! Project scaffolding, shared by `cargo xtask new` and the editor's
//! project launcher (so both write the same files).
//!
//! [`scaffold`] renders a **standalone** project — a game outside the
//! engine repository — into an empty directory:
//!
//! - `templates/standalone/` (workspace, the PC / N64 / 3DS hosts,
//!   `trino.toml`, AGENTS.md, starter assets);
//! - a game crate: the blank template (`templates/new-game/`), or a copy of
//!   one of the engine's demos ([`Demo`]) renamed to the new project, with
//!   the engine's `assets/`, `scenes/` and `prefabs/` it draws from.
//!
//! The engine crates come in as path dependencies on a local checkout
//! (default) or as git dependencies ([`EngineSource`]); the engine's
//! `Cargo.lock` is copied in, so a fresh project resolves the exact
//! dependency versions the engine is tested with.
//!
//! Template manifests are `Cargo.toml.tmpl` (a placeholder-laden
//! `Cargo.toml` would break cargo's scan of the engine as a git
//! dependency); [`template_target_name`] drops the suffix.

use std::ffi::{OsStr, OsString};
use std::path::{Component, Path, PathBuf};

/// A game name in the forms templates need.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Names {
    /// `my-game`: crate and file names.
    pub kebab: String,
    /// `my_game`: the crate's path in Rust.
    pub snake: String,
    /// `MyGame`: types (`MyGameGame`).
    pub camel: String,
}

impl Names {
    /// Kebab-case only: lowercase ASCII letters, digits and `-`, starting
    /// with a letter.
    pub fn new(name: &str) -> Result<Names, String> {
        let valid = !name.is_empty()
            && name
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
            && name.chars().next().is_some_and(|c| c.is_ascii_lowercase());
        if !valid {
            return Err(format!(
                "`{name}` is not a valid crate name — use kebab-case (e.g. my-game)"
            ));
        }
        let camel = name
            .split('-')
            .map(|w| {
                let mut c = w.chars();
                match c.next() {
                    Some(f) => f.to_ascii_uppercase().to_string() + c.as_str(),
                    None => String::new(),
                }
            })
            .collect();
        Ok(Names {
            kebab: name.to_string(),
            snake: name.replace('-', "_"),
            camel,
        })
    }

    /// Fill `{{name}}`, `{{name_snake}}` and `{{name_camel}}`.
    pub fn render(&self, text: &str) -> String {
        text.replace("{{name}}", &self.kebab)
            .replace("{{name_snake}}", &self.snake)
            .replace("{{name_camel}}", &self.camel)
    }
}

/// What the new project's game crate starts from.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Template {
    /// The minimal game: a sprite that moves and beeps.
    #[default]
    Blank,
    /// A copy of one of the engine's demo games.
    Demo(Demo),
}

/// Engine demos that make sense as a starting point.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Demo {
    /// `examples/castle64`: the 3D platformer (hub, stages, enemies, bot).
    Castle64,
    /// `examples/showcase`: the editor's demo scene exported to code, with
    /// the tag-driven scene runtime (player, coins, goal).
    Showcase,
    /// `examples/solid-eleven`: Solid Eleven, the low-poly football game
    /// (match sim, tactical AI, clubs, career, menus) with its own assets
    /// and console hosts whose self-test plays a scripted match.
    Football,
}

impl Demo {
    pub const ALL: [Demo; 3] = [Demo::Castle64, Demo::Showcase, Demo::Football];

    /// The `--template` value.
    pub fn key(self) -> &'static str {
        match self {
            Demo::Castle64 => "castle64",
            Demo::Showcase => "showcase",
            Demo::Football => "football",
        }
    }

    /// The demo's crate name (`examples/<crate>` in the engine).
    pub fn crate_name(self) -> &'static str {
        match self {
            Demo::Football => "solid-eleven",
            other => other.key(),
        }
    }

    /// The crate's path in Rust (`solid_eleven`).
    fn crate_snake(self) -> String {
        self.crate_name().replace('-', "_")
    }

    /// One line for pickers.
    pub fn about(self) -> &'static str {
        match self {
            Demo::Castle64 => "3D platformer: a castle hub, three stages, enemies, a HUD",
            Demo::Showcase => "an editor scene as code: a walking player, coins, a goal",
            Demo::Football => {
                "Solid Eleven: 3D football with full rules, tactical AI, clubs and a career"
            }
        }
    }

    /// The game type the demo defines (renamed to `<Camel>Game`).
    fn game_type(self) -> &'static str {
        match self {
            Demo::Castle64 => "Castle64Game",
            Demo::Showcase => "ShowcaseGame",
            Demo::Football => "SolidElevenGame",
        }
    }

    /// How the hosts build the game (`FnOnce(Vec2) -> G`), after renaming.
    fn constructor(self, names: &Names) -> String {
        match self {
            Demo::Castle64 | Demo::Football => format!("{}::{}Game::new", names.snake, names.camel),
            // `ShowcaseGame::new()` frames the screen itself.
            Demo::Showcase => format!("|_| {}::{}Game::new()", names.snake, names.camel),
        }
    }

    /// The demo ships its own `assets/` (instead of the engine's).
    fn own_assets(self) -> bool {
        self == Demo::Football
    }
}

impl Template {
    /// Every template, blank first.
    pub const ALL: [Template; 4] = [
        Template::Blank,
        Template::Demo(Demo::Castle64),
        Template::Demo(Demo::Showcase),
        Template::Demo(Demo::Football),
    ];

    /// `blank`, `castle64`, `showcase`, `football` (the `--template` values).
    pub fn key(self) -> &'static str {
        match self {
            Template::Blank => "blank",
            Template::Demo(d) => d.key(),
        }
    }

    pub fn parse(s: &str) -> Option<Template> {
        Template::ALL.into_iter().find(|t| t.key() == s)
    }

    /// One line for pickers.
    pub fn about(self) -> &'static str {
        match self {
            Template::Blank => "a sprite that moves and beeps: the smallest game",
            Template::Demo(d) => d.about(),
        }
    }

    /// `{{game_new}}`: the constructor expression the hosts pass to
    /// `trino_runner_pc::run`, `n64_main!` and `n3ds_main!`.
    fn constructor(self, names: &Names) -> String {
        match self {
            Template::Blank => format!("{}::{}Game::new", names.snake, names.camel),
            Template::Demo(d) => d.constructor(names),
        }
    }
}

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
    ("trino-platform-n64", "crates/platform-n64"),
    ("trino-platform-3ds", "crates/platform-3ds"),
    ("trino-platform-web", "crates/platform-web"),
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

/// Absolute, `..`-free path without `canonicalize` (which yields `\\?\`
/// verbatim paths on Windows that cargo cannot use as path dependencies).
pub fn absolute(path: &Path) -> Result<PathBuf, String> {
    let abs = std::path::absolute(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let mut out = PathBuf::new();
    for c in abs.components() {
        match c {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    Ok(out)
}

/// True when `dir` holds the engine's templates (a checkout scaffolding
/// can render from).
pub fn is_engine_checkout(dir: &Path) -> bool {
    dir.join("templates/standalone/trino.toml").is_file()
        && dir.join("templates/new-game/Cargo.toml.tmpl").is_file()
}

/// Render the standalone project into `dest` (must not exist or be empty).
pub fn scaffold(
    engine_root: &Path,
    dest: &Path,
    names: &Names,
    source: &EngineSource,
    template: Template,
) -> Result<(), String> {
    if !is_engine_checkout(engine_root) {
        return Err(format!(
            "{} is not a Trino engine checkout (no templates/): scaffolding needs one",
            engine_root.display()
        ));
    }
    if dest.exists()
        && std::fs::read_dir(dest)
            .map_err(|e| format!("{}: {e}", dest.display()))?
            .next()
            .is_some()
    {
        return Err(format!("{} exists and is not empty", dest.display()));
    }
    let deps = source.dependency_lines();
    // `cargo trino` runs the engine's xtask from a local checkout: this one
    // for path sources; for git sources the user points it at their clone.
    let engine_manifest = match source {
        EngineSource::Path(root) => root
            .join("Cargo.toml")
            .display()
            .to_string()
            .replace('\\', "/"),
        EngineSource::Git { .. } => "../Trino/Cargo.toml".to_string(),
    };
    let game_new = template.constructor(names);
    let render = |text: &str| {
        // `{{game_new}}` first: it expands to text with name placeholders
        // already filled.
        names
            .render(&text.replace("{{game_new}}", &game_new))
            .replace("{{trino_deps}}", &deps)
            .replace("{{engine_manifest}}", &engine_manifest)
    };
    let templates = engine_root.join("templates");
    copy_rendered(&templates.join("standalone"), dest, &render)?;
    // Dotfiles are stored without the dot (they would apply to the engine
    // repo itself otherwise).
    std::fs::rename(dest.join("gitignore"), dest.join(".gitignore")).map_err(|e| e.to_string())?;
    std::fs::rename(dest.join("cargo"), dest.join(".cargo")).map_err(|e| e.to_string())?;
    // Starter assets: the engine's own hero sprite and beep, and its pixel
    // font (the UI screens in `ui/` draw with it).
    for rel in ["sprites/hero.png", "sounds/beep.wav", "fonts/trino5x7.png"] {
        let to = dest.join("assets/shared").join(rel);
        std::fs::create_dir_all(to.parent().unwrap()).map_err(|e| e.to_string())?;
        std::fs::copy(engine_root.join("assets/shared").join(rel), &to)
            .map_err(|e| format!("{rel}: {e}"))?;
    }
    let game = dest.join("game");
    match template {
        Template::Blank => {
            // The same game template `xtask new` uses in-repo.
            copy_rendered(&templates.join("new-game/src"), &game.join("src"), &render)?;
            let manifest = read(&templates.join("new-game/Cargo.toml.tmpl"))?;
            write(&game.join("Cargo.toml"), &render(&manifest))?;
        }
        Template::Demo(demo) => copy_demo(engine_root, dest, demo, names)?,
    }
    // A template's own files over the standalone base (the football
    // template's hosts, test gates and guide).
    let overlay = templates.join(template.key());
    if overlay.is_dir() {
        copy_overlay(&overlay, dest, &render)?;
    }
    // Same dependency versions the engine is tested with.
    let lock = engine_root.join("Cargo.lock");
    if lock.exists() {
        std::fs::copy(&lock, dest.join("Cargo.lock")).map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// Rename a demo's Rust source to the new project: its game type and its
/// crate path. Nothing else changes — asset paths such as
/// `scenes/showcase` and `include!("showcase_gen.rs")` keep their names
/// (they name files and manifest keys, not the crate).
pub fn rename_demo_source(text: &str, demo: Demo, names: &Names) -> String {
    text.replace(demo.game_type(), &format!("{}Game", names.camel))
        .replace(
            &format!("{}::", demo.crate_snake()),
            &format!("{}::", names.snake),
        )
}

/// The demo's `Cargo.toml` as the new game crate's: the package renamed,
/// everything else (features, crate types) kept.
pub fn rename_demo_manifest(text: &str, demo: Demo, names: &Names) -> String {
    let from = format!("name = \"{}\"", demo.crate_name());
    let to = format!("name = \"{}\"", names.kebab);
    text.lines()
        .map(|l| if l.trim() == from { to.as_str() } else { l })
        .collect::<Vec<_>>()
        .join("\n")
        + "\n"
}

/// The demo's game crate (renamed) plus the content it draws from: its own
/// `assets/` when it has them (the football template), else all of the
/// engine's `assets/` (manifest included), `scenes/` and `prefabs/`.
fn copy_demo(engine: &Path, dest: &Path, demo: Demo, names: &Names) -> Result<(), String> {
    let src = engine.join("examples").join(demo.crate_name());
    let game = dest.join("game");
    let manifest = read(&src.join("Cargo.toml"))?;
    write(
        &game.join("Cargo.toml"),
        &rename_demo_manifest(&manifest, demo, names),
    )?;
    let note = format!(
        "//! {} — started from Trino's `{}` demo (`examples/{}` in the engine).\n//!\n",
        names.kebab,
        demo.key(),
        demo.crate_name()
    );
    copy_tree(&src.join("src"), &game.join("src"), &mut |rel, bytes| {
        if rel.extension() != Some(OsStr::new("rs")) {
            return bytes;
        }
        let text = String::from_utf8_lossy(&bytes);
        let mut out = rename_demo_source(&text, demo, names);
        if rel == Path::new("lib.rs") {
            out.insert_str(0, &note);
        }
        out.into_bytes()
    })?;
    if demo.own_assets() {
        // Replace the starter manifest with the demo's, keep the starter
        // masters next to its own.
        copy_tree(&src.join("assets"), &dest.join("assets"), &mut |_, b| b)?;
        // The starter UI screens (`ui/`) draw with the engine's font.
        let manifest = dest.join("assets/manifest.toml");
        let text = read(&manifest)?;
        if !text.contains("[fonts.trino5x7]") {
            write(&manifest, &format!("{text}{UI_FONT_DECL}"))?;
        }
        return Ok(());
    }
    for dir in ["assets", "scenes", "prefabs", "ui"] {
        let from = engine.join(dir);
        if from.is_dir() {
            copy_tree(&from, &dest.join(dir), &mut |_, b| b)?;
        }
    }
    Ok(())
}

/// The engine font's manifest entry, for a demo manifest that lacks it.
const UI_FONT_DECL: &str = "\n# The engine's pixel font: what the UI screens in ui/ draw with.\n\
                            [fonts.trino5x7]\nfile = \"fonts/trino5x7.png\"\ncell = [6, 10]\n";

/// Copy a template overlay over the project: text files rendered like the
/// standalone base (`*.tmpl` suffix dropped), others byte for byte.
fn copy_overlay(from: &Path, to: &Path, render: &dyn Fn(&str) -> String) -> Result<(), String> {
    copy_tree(
        from,
        to,
        &mut |rel, bytes| match std::str::from_utf8(&bytes) {
            Ok(text) if rel.extension() != Some(OsStr::new("png")) => render(text).into_bytes(),
            _ => bytes,
        },
    )?;
    // `copy_tree` keeps names: drop `.tmpl` suffixes after the fact.
    fn untmpl(dir: &Path) -> Result<(), String> {
        for e in std::fs::read_dir(dir).map_err(|e| e.to_string())? {
            let p = e.map_err(|e| e.to_string())?.path();
            if p.is_dir() {
                untmpl(&p)?;
            } else if p.extension() == Some(OsStr::new("tmpl")) {
                let target = p.with_file_name(template_target_name(p.file_name().unwrap()));
                std::fs::rename(&p, &target).map_err(|e| e.to_string())?;
            }
        }
        Ok(())
    }
    untmpl(to)
}

/// Template files that would confuse tools in place (a placeholder-laden
/// `Cargo.toml` breaks cargo's git-source scan of this repo) carry a
/// `.tmpl` suffix, dropped when rendered.
pub fn template_target_name(name: &OsStr) -> OsString {
    let s = name.to_string_lossy();
    match s.strip_suffix(".tmpl") {
        Some(base) => base.into(),
        None => name.to_owned(),
    }
}

fn read(path: &Path) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
}

fn write(path: &Path, text: &str) -> Result<(), String> {
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(path, text).map_err(|e| format!("{}: {e}", path.display()))
}

/// Copy a template directory, rendering every file as text.
pub fn copy_rendered(
    from: &Path,
    to: &Path,
    render: &dyn Fn(&str) -> String,
) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for entry in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let entry = entry.map_err(|e| e.to_string())?;
        let src = entry.path();
        let dst = to.join(template_target_name(&entry.file_name()));
        if src.is_dir() {
            copy_rendered(&src, &dst, render)?;
        } else {
            write(&dst, &render(&read(&src)?))?;
        }
    }
    Ok(())
}

/// Copy a directory tree byte for byte, through `map(relative path, bytes)`.
fn copy_tree(
    from: &Path,
    to: &Path,
    map: &mut dyn FnMut(&Path, Vec<u8>) -> Vec<u8>,
) -> Result<(), String> {
    fn walk(
        base: &Path,
        dir: &Path,
        to: &Path,
        map: &mut dyn FnMut(&Path, Vec<u8>) -> Vec<u8>,
    ) -> Result<(), String> {
        for entry in std::fs::read_dir(dir).map_err(|e| format!("{}: {e}", dir.display()))? {
            let path = entry.map_err(|e| e.to_string())?.path();
            let rel = path.strip_prefix(base).unwrap();
            if path.is_dir() {
                walk(base, &path, to, map)?;
            } else {
                let bytes = std::fs::read(&path).map_err(|e| format!("{}: {e}", path.display()))?;
                let out = to.join(rel);
                if let Some(d) = out.parent() {
                    std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
                }
                std::fs::write(&out, map(rel, bytes))
                    .map_err(|e| format!("{}: {e}", out.display()))?;
            }
        }
        Ok(())
    }
    walk(from, from, to, map)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> PathBuf {
        absolute(&Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")).unwrap()
    }

    /// A temp dir under the engine's `target/` (never the system temp dir).
    fn temp() -> tempfile::TempDir {
        let base = engine().join("target/test-projects");
        std::fs::create_dir_all(&base).unwrap();
        tempfile::tempdir_in(base).unwrap()
    }

    fn files(dir: &Path) -> Vec<PathBuf> {
        let mut out = Vec::new();
        copy_tree_list(dir, dir, &mut out);
        out.sort();
        out
    }

    fn copy_tree_list(base: &Path, dir: &Path, out: &mut Vec<PathBuf>) {
        for e in std::fs::read_dir(dir).unwrap() {
            let p = e.unwrap().path();
            if p.is_dir() {
                copy_tree_list(base, &p, out);
            } else {
                out.push(p.strip_prefix(base).unwrap().to_path_buf());
            }
        }
    }

    #[test]
    fn names_take_every_form() {
        let n = Names::new("my-game2").unwrap();
        assert_eq!(
            (n.kebab.as_str(), n.snake.as_str(), n.camel.as_str()),
            ("my-game2", "my_game2", "MyGame2")
        );
        assert_eq!(
            n.render("{{name}} {{name_snake}} {{name_camel}}"),
            "my-game2 my_game2 MyGame2"
        );
        for bad in ["", "My-Game", "my_game", "1game", "-game", "my game"] {
            assert!(Names::new(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn templates_parse_their_keys() {
        for t in Template::ALL {
            assert_eq!(Template::parse(t.key()), Some(t));
            assert!(!t.about().is_empty());
        }
        assert_eq!(Template::parse("platformer"), None);
        assert_eq!(Template::default(), Template::Blank);
    }

    /// The rename touches the demo's game type and crate path only: asset
    /// paths, generated-code file names and the rest stay.
    #[test]
    fn demo_renames_touch_only_the_crate_and_its_type() {
        let n = Names::new("my-game").unwrap();
        let src = "use crate::Castle64Game;\nfn f() -> castle64::Castle64Game { todo!() }\n\
                   const M: ModelId = ModelId::from_path(\"models/c64_boar\");\n";
        let out = rename_demo_source(src, Demo::Castle64, &n);
        assert_eq!(
            out,
            "use crate::MyGameGame;\nfn f() -> my_game::MyGameGame { todo!() }\n\
             const M: ModelId = ModelId::from_path(\"models/c64_boar\");\n"
        );
        let src = "include!(\"showcase_gen.rs\");\nBlobId::from_path(\"scenes/showcase\");\n\
                   impl Default for ShowcaseGame {}\nshowcase::ShowcaseGame::new()\n";
        let out = rename_demo_source(src, Demo::Showcase, &n);
        assert!(out.contains("include!(\"showcase_gen.rs\")"), "{out}");
        assert!(out.contains("from_path(\"scenes/showcase\")"), "{out}");
        assert!(out.contains("impl Default for MyGameGame"), "{out}");
        assert!(out.contains("my_game::MyGameGame::new()"), "{out}");

        let manifest = "[package]\nname = \"showcase\"\ndescription = \"showcase demo\"\n\n\
                        [features]\nhot-reload = []\n";
        let out = rename_demo_manifest(manifest, Demo::Showcase, &n);
        assert!(out.contains("name = \"my-game\"\n"), "{out}");
        assert!(out.contains("description = \"showcase demo\""), "{out}");
        assert!(out.contains("hot-reload = []"), "{out}");
    }

    /// Both kinds of project, rendered: every file, no placeholder left,
    /// the hosts calling the renamed game, the demo's content copied.
    #[test]
    fn scaffolds_blank_and_demo_projects() {
        let root = engine();
        let src = EngineSource::Path(root.clone());
        let tmp = temp();
        let n = Names::new("my-game").unwrap();
        for template in Template::ALL {
            let dest = tmp.path().join(template.key());
            scaffold(&root, &dest, &n, &src, template).unwrap();
            for f in [
                "Cargo.toml",
                "Cargo.lock",
                ".gitignore",
                ".cargo/config.toml",
                "AGENTS.md",
                "trino.toml",
                "game/Cargo.toml",
                "game/src/lib.rs",
                "pc/src/main.rs",
                "n64/src/lib.rs",
                "n3ds/src/main.rs",
                "web/Cargo.toml",
                "web/src/lib.rs",
                "assets/manifest.toml",
                "assets/shared/sprites/hero.png",
                "assets/shared/fonts/trino5x7.png",
                "assets/i18n/en-US.ron",
                "ui/title.ui.ron",
                "ui/pause.ui.ron",
                "ui/options.ui.ron",
                "ui/controls.ui.ron",
                "ui/accessibility.ui.ron",
            ] {
                assert!(dest.join(f).is_file(), "{}: {f}", template.key());
            }
            for f in files(&dest) {
                let Ok(text) = std::fs::read_to_string(dest.join(&f)) else {
                    continue; // binary assets
                };
                assert!(
                    !text.contains("{{"),
                    "{}: {} keeps a placeholder",
                    template.key(),
                    f.display()
                );
                if let Template::Demo(d) = template
                    && f.starts_with("game")
                {
                    assert!(!text.contains(d.game_type()), "{}", f.display());
                    assert!(
                        !text.contains(&format!("{}::", d.crate_snake())),
                        "{}",
                        f.display()
                    );
                }
            }
            let game = std::fs::read_to_string(dest.join("game/Cargo.toml")).unwrap();
            assert!(game.contains("name = \"my-game\""), "{game}");
            // The renamed game type, wherever the crate defines it.
            let defined = files(&dest.join("game/src")).iter().any(|f| {
                std::fs::read_to_string(dest.join("game/src").join(f))
                    .is_ok_and(|t| t.contains("pub struct MyGameGame"))
            });
            assert!(defined, "{}", template.key());
            // The hosts are cfg'd to their console, so a host `cargo check`
            // never sees their bodies: check the text instead.
            let ctor = match template {
                Template::Demo(Demo::Showcase) => "|_| my_game::MyGameGame::new()",
                _ => "my_game::MyGameGame::new",
            };
            for host in [
                "pc/src/main.rs",
                "n64/src/lib.rs",
                "n3ds/src/main.rs",
                "web/src/lib.rs",
            ] {
                let text = std::fs::read_to_string(dest.join(host)).unwrap();
                assert!(text.contains(ctor), "{}: {host}:\n{text}", template.key());
            }
            for host in ["n64/src/lib.rs", "n3ds/src/main.rs"] {
                let text = std::fs::read_to_string(dest.join(host)).unwrap();
                assert!(text.contains("game: &mut my_game::MyGameGame"), "{host}");
            }
        }
        let c64 = tmp.path().join("castle64");
        for f in [
            "game/src/bot.rs",
            "game/src/levels.rs",
            "scenes/showcase.scene.ron",
        ] {
            assert!(c64.join(f).is_file(), "{f}");
        }
        let manifest = std::fs::read_to_string(c64.join("assets/manifest.toml")).unwrap();
        assert!(
            manifest.contains("[models.c64_boar]"),
            "the engine's manifest"
        );
        let show = tmp.path().join("showcase");
        assert!(show.join("game/src/showcase_gen.rs").is_file());
        let game = std::fs::read_to_string(show.join("game/Cargo.toml")).unwrap();
        assert!(game.contains("hot-reload = []"), "{game}");
        // Football: its own assets (not the engine's), its console hosts
        // with the scripted-match self-test, its gates.
        let fb = tmp.path().join("football");
        for f in [
            "game/src/sim/engine.rs",
            "game/src/selftest.rs",
            "assets/shared/models/pitch_0_0.glb",
            "assets/shared/fonts/RobotoCondensed-Bold.ttf",
        ] {
            assert!(fb.join(f).is_file(), "football: {f}");
        }
        let manifest = std::fs::read_to_string(fb.join("assets/manifest.toml")).unwrap();
        assert!(
            manifest.contains("[models.se_pitch_0_0]"),
            "its own manifest"
        );
        assert!(
            !manifest.contains("c64_"),
            "no castle64 assets in a football project"
        );
        assert!(!fb.join("assets/shared/models/castle64").exists());
        let n64 = std::fs::read_to_string(fb.join("n64/src/lib.rs")).unwrap();
        assert!(
            n64.contains("my_game::selftest") || n64.contains("use my_game::selftest"),
            "{n64}"
        );
        let n3ds = std::fs::read_to_string(fb.join("n3ds/src/main.rs")).unwrap();
        assert!(n3ds.contains("dump_framebuffer_bottom"), "{n3ds}");
        let toml = std::fs::read_to_string(fb.join("trino.toml")).unwrap();
        assert!(toml.contains("n64_goldens = [\"match\"]"), "{toml}");
        assert!(toml.contains("name = \"my-game\""), "{toml}");
        let blank = tmp.path().join("blank");
        assert!(
            !blank.join("scenes").exists(),
            "blank copies no demo content"
        );
        // Not empty any more: refused.
        let e = scaffold(&root, &blank, &n, &src, Template::Blank).unwrap_err();
        assert!(e.contains("exists and is not empty"), "{e}");
        // Not an engine checkout: refused before writing anything.
        let e = scaffold(tmp.path(), &tmp.path().join("x"), &n, &src, Template::Blank).unwrap_err();
        assert!(e.contains("not a Trino engine checkout"), "{e}");
        assert!(!tmp.path().join("x").exists());
    }
}
