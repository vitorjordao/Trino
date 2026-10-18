//! `cargo xtask` end to end: the real binary, driven like a contributor or
//! CI drives it — help, argument errors, project selection (`--project`,
//! `$TRINO_PROJECT`, a `trino.toml` in the cwd), `agents check`,
//! `assets pc` and the `new` scaffolder's standalone mode.
//!
//! Only commands that read the repository or write where they are told run
//! here; console builds, emulators, Blender and the in-repo scaffolder are
//! listed as untested in `docs/TESTING.md`.

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Run xtask from the repo root, with no project selected from the
/// environment.
fn xtask_in(cwd: &Path, args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(args)
        .current_dir(cwd)
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn xtask(args: &[&str]) -> Output {
    xtask_in(&root(), args)
}

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

/// Fails with `needle` on stderr.
fn assert_fails_with(o: &Output, needle: &str) {
    assert!(
        !o.status.success(),
        "expected failure, got success: {}",
        stdout(o)
    );
    assert!(
        stderr(o).contains(needle),
        "stderr lacks `{needle}`:\n{}",
        stderr(o)
    );
}

#[test]
fn help_and_no_arguments_list_every_command() {
    let help = xtask(&["help"]);
    assert!(help.status.success());
    let text = stdout(&help);
    for cmd in [
        "build <pc|n64|3ds|web>",
        "run   <pc|n64|3ds|web>",
        "test  [pc|n64|3ds|web]",
        "assets <pc|n64|3ds|web>",
        "build web [--release] [--out <dir>]",
        "run web [--release] [--port N]",
        "test web [--headed]",
        "watch <pc|n64>",
        "editor",
        "emulators setup ares|azahar",
        "new <name>",
        "gen-assets",
        "scene check",
        "scene export",
        "model inspect",
        "blender smoke",
        "agents <sync|check>",
        "--project <dir>",
        "build [all]",
        "assets [all]",
        "test all",
        "[project] platforms",
        "editor [<dir>]",
        "run n64 --device sc64|everdrive",
        "run 3ds --device network",
        "run|test|watch n64 --expansion-pak on|off",
        "run|test|watch 3ds --new3ds on|off",
        "build|run|test|watch 3ds --speedup on|off",
    ] {
        assert!(text.contains(cmd), "help lacks `{cmd}`");
    }
    let bare = xtask(&[]);
    assert!(bare.status.success());
    assert_eq!(stdout(&bare), text, "no arguments prints the same help");
}

#[test]
fn unknown_invocations_fail_with_the_usage() {
    let o = xtask(&["frobnicate"]);
    assert_fails_with(&o, "unknown invocation `frobnicate`");
    assert!(stderr(&o).contains("commands:"), "usage follows the error");
    assert_fails_with(
        &xtask(&["build", "xbox"]),
        "unknown invocation `build xbox`",
    );
    assert_fails_with(
        &xtask(&["assets", "gamecube"]),
        "unknown platform `gamecube`",
    );
    assert_fails_with(
        &xtask(&["watch", "pc", "--game"]),
        "--game needs a crate name",
    );
}

/// The hardware-tier switches (issue #54) fail fast, before any toolchain
/// runs, when their value or their command is wrong.
#[test]
fn hardware_tier_flags_go_with_their_console() {
    assert_fails_with(
        &xtask(&["test", "n64", "--expansion-pak", "maybe"]),
        "--expansion-pak takes on, off or both",
    );
    assert_fails_with(
        &xtask(&["build", "n64", "--expansion-pak", "off"]),
        "--expansion-pak goes with `run|test|watch n64`",
    );
    assert_fails_with(
        &xtask(&["test", "3ds", "--expansion-pak", "off"]),
        "--expansion-pak goes with `run|test|watch n64`",
    );
    assert_fails_with(
        &xtask(&["run", "3ds", "--new3ds", "both"]),
        "only `test 3ds` boots twice",
    );
    assert_fails_with(
        &xtask(&["test", "n64", "--new3ds", "off"]),
        "--new3ds goes with `run|test|watch 3ds`",
    );
    assert_fails_with(
        &xtask(&["build", "n64", "--speedup", "off"]),
        "--speedup goes with `build|run|test|watch 3ds`",
    );
    assert_fails_with(
        &xtask(&["build", "3ds", "--speedup", "both"]),
        "--speedup takes on or off",
    );
    assert_fails_with(
        &xtask(&["run", "n64", "--device", "sc64", "--expansion-pak", "off"]),
        "real hardware has the Pak it has",
    );
}

/// Tool provisioning commands validate their arguments before touching
/// Docker, the network or Blender.
#[test]
fn emulator_and_blender_commands_reject_unknown_subcommands() {
    for args in [
        &["emulators"][..],
        &["emulators", "setup"],
        &["emulators", "setup", "mame"],
    ] {
        assert_fails_with(
            &xtask(args),
            "usage: cargo xtask emulators setup ares|azahar",
        );
    }
    for args in [&["blender"][..], &["blender", "render"]] {
        assert_fails_with(&xtask(args), "usage: cargo xtask blender smoke");
    }
}

/// A console command is never reached when the project cannot be selected:
/// every failure below happens before any toolchain is touched (`help` is
/// the command, so a passing selection only prints the usage).
#[test]
fn project_selection_fails_fast_on_bad_projects() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    let s = dir.to_str().unwrap();

    assert_fails_with(
        &xtask(&["help", "--project"]),
        "--project needs a directory",
    );
    assert_fails_with(
        &xtask(&["help", "--project", "/no/such/project"]),
        "project /no/such/project",
    );
    assert_fails_with(&xtask(&["help", "--project", s]), "trino.toml");

    let write = |text: &str| std::fs::write(dir.join("trino.toml"), text).unwrap();
    let good = "[project]\nname = \"demo-game\"\ntitle = \"Demo\"\n\n\
                [n64]\npackage = \"demo-n64\"\n\n[3ds]\npackage = \"demo-3ds\"\nbin = \"demo-3ds\"\n";
    write(&good.replace("demo-game", "demo game"));
    assert_fails_with(
        &xtask(&["help", "--project", s]),
        "project.name `demo game`",
    );
    write(&good.replace("\"Demo\"", "\"A title far too long for a ROM\""));
    assert_fails_with(&xtask(&["help", "--project", s]), "≤ 20 ASCII characters");
    write(&format!("{good}\n[test]\nunknown_gate = 1\n"));
    assert_fails_with(&xtask(&["help", "--project", s]), "unknown_gate");

    // The same broken file is found through $TRINO_PROJECT and through a
    // trino.toml in the working directory.
    let env = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("help")
        .current_dir(root())
        .env("TRINO_PROJECT", dir)
        .output()
        .unwrap();
    assert_fails_with(&env, "unknown_gate");
    assert_fails_with(&xtask_in(dir, &["help"]), "unknown_gate");

    write(good);
    let ok = xtask(&["help", "--project", s]);
    assert!(ok.status.success(), "{}", stderr(&ok));
    assert!(stdout(&ok).contains("commands:"));
}

/// A throwaway project under this repo's `target/` (never `/tmp`, a RAM
/// disk on some machines) with `platforms` set and one sound to bake.
fn platforms_project(platforms: &str) -> tempfile::TempDir {
    let base = root().join("target/test-projects");
    std::fs::create_dir_all(&base).unwrap();
    let tmp = tempfile::tempdir_in(&base).unwrap();
    std::fs::write(
        tmp.path().join("trino.toml"),
        format!("[project]\nname = \"my-game\"\ntitle = \"My Game\"\nplatforms = {platforms}\n"),
    )
    .unwrap();
    let sounds = tmp.path().join("assets/shared/sounds");
    std::fs::create_dir_all(&sounds).unwrap();
    std::fs::copy(
        root().join("assets/shared/sounds/beep.wav"),
        sounds.join("beep.wav"),
    )
    .unwrap();
    std::fs::write(
        tmp.path().join("assets/manifest.toml"),
        "version = 1\n\n[sounds.beep]\nfile = \"sounds/beep.wav\"\n",
    )
    .unwrap();
    tmp
}

/// `[project] platforms`: naming a platform the project leaves out fails
/// before any toolchain runs, and the message names the field and the
/// file; a manifest without the field still accepts every platform.
#[test]
fn platform_commands_follow_the_project_platforms() {
    let tmp = platforms_project(r#"["pc"]"#);
    let s = tmp.path().to_str().unwrap();
    for args in [
        &["build", "n64"][..],
        &["run", "n64"],
        &["test", "n64"],
        &["watch", "n64"],
        &["assets", "n64"],
        &["build", "3ds"],
        &["test", "n3ds"],
        &["assets", "3ds"],
    ] {
        let mut a = args.to_vec();
        a.extend(["--project", s]);
        let o = xtask(&a);
        let platform = if args[1] == "n64" { "n64" } else { "3ds" };
        assert_fails_with(&o, &format!("does not target {platform}"));
        assert_fails_with(&o, "[project] platforms = [\"pc\"]");
        assert_fails_with(&o, "trino.toml");
    }
    // Through the cwd's trino.toml too (what `cargo trino` does).
    assert_fails_with(
        &xtask_in(tmp.path(), &["build", "n64"]),
        "does not target n64",
    );

    // The batch bake does the PC and says what it skipped.
    let o = xtask(&["assets", "all", "--project", s]);
    assert!(o.status.success(), "{}", stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("baked 1 asset(s) for pc"), "{out}");
    assert!(
        out.contains("skipping n64 (not in [project] platforms)"),
        "{out}"
    );
    assert!(
        out.contains("skipping 3ds (not in [project] platforms)"),
        "{out}"
    );
    assert!(tmp.path().join("target/assets/pc/index.toml").is_file());
    assert!(!tmp.path().join("target/assets/n64").exists());

    // A console-only project refuses the PC commands the same way.
    let consoles = platforms_project(r#"["pc"]"#); // trino.toml rewritten:
    std::fs::write(
        consoles.path().join("trino.toml"),
        "[project]\nname = \"my-game\"\ntitle = \"My Game\"\nplatforms = [\"n64\"]\n\n\
         [n64]\npackage = \"my-game-n64\"\n",
    )
    .unwrap();
    let c = consoles.path().to_str().unwrap();
    assert_fails_with(
        &xtask(&["assets", "pc", "--project", c]),
        "does not target pc",
    );
    assert_fails_with(
        &xtask(&["build", "pc", "--project", c]),
        "does not target pc",
    );
}

#[test]
fn agents_check_passes_on_the_clean_tree_and_rejects_unknown_subcommands() {
    let o = xtask(&["agents", "check"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("mirrors are in sync"));
    assert_fails_with(
        &xtask(&["agents"]),
        "usage: cargo xtask agents <sync|check>",
    );
    assert_fails_with(
        &xtask(&["agents", "frob"]),
        "usage: cargo xtask agents <sync|check>",
    );
}

/// `assets web`: the Web bake (issue #47), KTX2 beside the masters.
#[test]
fn assets_web_bakes_ktx2_textures() {
    let o = xtask(&["assets", "web"]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("for web into"), "{}", stdout(&o));
    let dir = root().join("target/assets/web");
    let index = std::fs::read_to_string(dir.join("index.toml")).unwrap();
    let ktx2: Vec<&str> = index
        .lines()
        .filter_map(|l| l.strip_prefix("ktx2 = \"")?.strip_suffix('"'))
        .collect();
    assert!(!ktx2.is_empty(), "no KTX2 texture in the Web bake");
    for f in ktx2 {
        let bytes = std::fs::read(dir.join(f)).unwrap();
        trino_asset_pipeline::texture::ktx2::parse(&bytes).unwrap_or_else(|e| panic!("{f}: {e}"));
    }
}

#[test]
fn assets_pc_bakes_the_repo_manifest() {
    let o = xtask(&["assets", "pc"]);
    assert!(o.status.success(), "{}", stderr(&o));
    let line = stdout(&o);
    let count: usize = line
        .strip_prefix("baked ")
        .and_then(|r| r.split_whitespace().next())
        .and_then(|n| n.parse().ok())
        .unwrap_or_else(|| panic!("unexpected output: {line}"));
    assert!(line.contains("for pc into"), "{line}");
    let manifest = std::fs::read_to_string(root().join("assets/manifest.toml")).unwrap();
    let declared = |kinds: &[&str]| {
        manifest
            .lines()
            .filter(|l| kinds.iter().any(|k| l.starts_with(&format!("[{k}."))))
            .count()
    };
    // Models named only for another platform or a console tier (issue #54)
    // are not the PC's.
    let parsed =
        trino_asset_pipeline::Manifest::load(&root().join("assets/manifest.toml")).unwrap();
    let not_pc = parsed
        .models
        .values()
        .filter(|d| !d.bakes_for(trino_asset_pipeline::Platform::Pc))
        .count();
    assert!(not_pc > 0, "the self-test's tier-only model");
    let all = declared(&[
        "sprites",
        "sounds",
        "music",
        "models",
        "fonts",
        "blobs",
        "scenes",
        "particles",
    ]) - not_pc;
    // A font bakes twice under one handle: atlas sprite + glyph table; the
    // project mix (`assets/audio.toml`) is one more asset, `audio/mixer`.
    let fonts = declared(&["fonts"]);
    assert!(fonts > 0);
    let mix = root().join("assets/audio.toml").exists() as usize;
    assert_eq!(count, all + fonts + mix, "every manifest entry is baked");
    let index = std::fs::read_to_string(root().join("target/assets/pc/index.toml")).unwrap();
    assert_eq!(index.matches("[[asset]]").count(), count);
    let kind = |k: &str| index.matches(&format!("kind = \"{k}\"")).count();
    assert_eq!(kind("model"), declared(&["models"]) - not_pc);
    assert_eq!(kind("sound"), declared(&["sounds"]));
    assert_eq!(kind("music"), declared(&["music"]));
    assert_eq!(kind("font"), fonts);
    assert_eq!(kind("mixer"), mix);
    assert_eq!(kind("sprite"), declared(&["sprites"]) + fonts);
    // Baked scenes (TSCN) and particle effects (TPFX) are blobs, served
    // like any other blob.
    assert_eq!(kind("blob"), declared(&["blobs", "scenes", "particles"]));
}

#[test]
fn new_rejects_bad_names_and_bad_options() {
    assert_fails_with(&xtask(&["new"]), "usage: cargo xtask new <kebab-case-name>");
    assert_fails_with(&xtask(&["new", "Bad_Name"]), "not a valid crate name");
    assert!(!root().join("examples/Bad_Name").exists());
    assert_fails_with(
        &xtask(&["new", "demo", "--git", "https://example.invalid/t.git"]),
        "need --standalone",
    );
    assert_fails_with(
        &xtask(&["new", "demo", "--standalone"]),
        "--standalone needs a value",
    );
    assert_fails_with(
        &xtask(&["new", "demo", "--frob"]),
        "unknown option `--frob`",
    );
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("p");
    let d = dir.to_str().unwrap();
    assert_fails_with(
        &xtask(&["new", "demo", "--standalone", d, "--rev", "abc"]),
        "--rev/--branch need --git",
    );
    assert_fails_with(
        &xtask(&["new", "Bad", "--standalone", d]),
        "not a valid crate name",
    );
    assert!(!dir.exists(), "nothing is written on a bad name");
}

/// Every file under `dir`, recursively.
fn walk(dir: &Path, out: &mut Vec<PathBuf>) {
    for e in std::fs::read_dir(dir).unwrap() {
        let p = e.unwrap().path();
        if p.is_dir() {
            walk(&p, out);
        } else {
            out.push(p);
        }
    }
}

/// The scaffold only (the slow build-and-run is `standalone.rs`): a
/// complete project with no template placeholders left, engine crates as
/// path dependencies on this checkout or as pinned git dependencies.
#[test]
fn new_standalone_scaffolds_a_complete_project() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("moon-hop");
    let d = dir.to_str().unwrap();
    let o = xtask(&["new", "moon-hop", "--standalone", d]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("created standalone project"));

    for rel in [
        "Cargo.toml",
        "Cargo.lock",
        "AGENTS.md",
        "trino.toml",
        "rust-toolchain.toml",
        ".gitignore",
        ".cargo/config.toml",
        "assets/manifest.toml",
        "assets/shared/sprites/hero.png",
        "assets/shared/sounds/beep.wav",
        "game/Cargo.toml",
        "game/src/lib.rs",
        "pc/Cargo.toml",
        "pc/src/main.rs",
        "n64/Cargo.toml",
        "n3ds/Cargo.toml",
    ] {
        assert!(dir.join(rel).is_file(), "{rel} missing");
    }
    let mut files = Vec::new();
    walk(&dir, &mut files);
    for f in files.iter().filter(|f| {
        !matches!(
            f.extension().and_then(|e| e.to_str()),
            Some("png" | "wav" | "lock")
        )
    }) {
        let text = std::fs::read_to_string(f).unwrap();
        assert!(!text.contains("{{"), "{} keeps a placeholder", f.display());
        assert!(
            !f.to_string_lossy().ends_with(".tmpl"),
            "{} kept its .tmpl suffix",
            f.display()
        );
    }
    let game = std::fs::read_to_string(dir.join("game/Cargo.toml")).unwrap();
    assert!(game.contains("name = \"moon-hop\""), "{game}");
    let lib = std::fs::read_to_string(dir.join("game/src/lib.rs")).unwrap();
    assert!(lib.contains("MoonHop"), "camel-case game type");
    let ws = std::fs::read_to_string(dir.join("Cargo.toml")).unwrap();
    let engine = std::path::absolute(root()).unwrap();
    let core = trino_dep_path(&ws);
    assert_eq!(
        std::fs::canonicalize(core).unwrap(),
        std::fs::canonicalize(engine.join("crates/core")).unwrap(),
        "path dependency on this checkout"
    );
    assert_eq!(
        std::fs::read(dir.join("Cargo.lock")).unwrap(),
        std::fs::read(root().join("Cargo.lock")).unwrap(),
        "the engine's lock file is copied"
    );
    // The scaffolded trino.toml is a valid project for the console commands.
    let sel = xtask(&["help", "--project", d]);
    assert!(sel.status.success(), "{}", stderr(&sel));

    // A non-empty destination is refused.
    assert_fails_with(
        &xtask(&["new", "moon-hop", "--standalone", d]),
        "exists and is not empty",
    );

    // Git sources pin a branch (or rev) on every engine crate.
    let git_dir = tmp.path().join("git-hop");
    let g = git_dir.to_str().unwrap();
    let o = xtask(&[
        "new",
        "git-hop",
        "--standalone",
        g,
        "--git",
        "https://example.invalid/trino.git",
        "--branch",
        "main",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    let ws = std::fs::read_to_string(git_dir.join("Cargo.toml")).unwrap();
    for krate in ["trino-core", "trino-game-api", "trino-runner-pc"] {
        assert!(
            ws.contains(&format!(
                "{krate} = {{ git = \"https://example.invalid/trino.git\", branch = \"main\" }}"
            )),
            "{krate} git dependency:\n{ws}"
        );
    }
}

/// `--template` copies a demo; `new` with no name asks on stdin (name,
/// folder, template) and builds the same project.
#[test]
fn new_from_a_demo_template_and_from_the_prompts() {
    let base = root().join("target/test-projects");
    std::fs::create_dir_all(&base).unwrap();
    let tmp = tempfile::tempdir_in(&base).unwrap();
    let dir = tmp.path().join("castle-copy");
    let d = dir.to_str().unwrap();
    let o = xtask(&[
        "new",
        "castle-copy",
        "--standalone",
        d,
        "--template",
        "castle64",
    ]);
    assert!(o.status.success(), "{}", stderr(&o));
    assert!(stdout(&o).contains("(castle64 template)"), "{}", stdout(&o));
    let lib = std::fs::read_to_string(dir.join("game/src/lib.rs")).unwrap();
    assert!(
        lib.contains("pub struct CastleCopyGame"),
        "renamed game type"
    );
    assert!(dir.join("game/src/bot.rs").is_file());
    assert_fails_with(
        &xtask(&["new", "x", "--standalone", d, "--template", "doom"]),
        "unknown template `doom`",
    );
    assert_fails_with(
        &xtask(&["new", "x", "--template", "showcase"]),
        "--template needs --standalone",
    );

    let asked = tmp.path().join("asked");
    let mut child = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .arg("new")
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    {
        use std::io::Write;
        let mut stdin = child.stdin.take().unwrap();
        writeln!(stdin, "Not Valid\nmy-game\n{}\nshowcase", asked.display()).unwrap();
    }
    let o = child.wait_with_output().unwrap();
    assert!(o.status.success(), "{}{}", stdout(&o), stderr(&o));
    let out = stdout(&o);
    assert!(out.contains("not a valid crate name"), "{out}");
    assert!(out.contains("(showcase template)"), "{out}");
    assert!(asked.join("game/src/showcase_gen.rs").is_file());
    let pc = std::fs::read_to_string(asked.join("pc/src/main.rs")).unwrap();
    assert!(pc.contains("|_| my_game::MyGameGame::new()"), "{pc}");
}

/// The `path = "..."` of `trino-core` in a workspace manifest.
fn trino_dep_path(ws: &str) -> PathBuf {
    let line = ws
        .lines()
        .find(|l| l.starts_with("trino-core = "))
        .unwrap_or_else(|| panic!("no trino-core dependency:\n{ws}"));
    let start = line.find("path = \"").expect("a path dependency") + 8;
    let end = line[start..].find('"').unwrap() + start;
    PathBuf::from(&line[start..end])
}
