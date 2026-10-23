//! `cargo xtask package` (issue #61, ADR 0029, docs/SHIPPING.md): a game
//! as players get it.
//!
//! `package pc [--target linux|windows|macos] [--steam] [--itch] [--out
//! <dir>] [--smoke] [--json]` builds a project's PC host (`[pc] package`)
//! with the `dist` profile (release, fat LTO, one codegen unit), stamps the
//! version (`TRINO_GAME_VERSION`, `TRINO_GIT_COMMIT`, `TRINO_BUILD_DATE`:
//! `trino_core::build_info!()`), bakes the assets into the package, writes
//! `trino-package.txt` (the runner's packaged mode) and lays it out per OS:
//!
//! - **Linux:** `<name>-<version>-linux-x86_64.AppImage` (a pinned type-2
//!   runtime + `mksquashfs`), and the plain folder as a `.zip`.
//! - **Windows:** `<name>-<version>-windows-x86_64.zip` with the `.exe`
//!   carrying the icon and the version information (a `.res` written by
//!   [`winres`]) and the GUI subsystem.
//! - **macOS:** `<Title>.app` (`Info.plist`, `.icns`) zipped, and a `.dmg`
//!   on a Mac (`hdiutil`).
//!
//! `--steam` also writes the SteamPipe layout (`dist/steam`: `content/<os>`
//! and `scripts/app_build_<app>.vdf`), `--itch` butler's (`dist/itch/<os>`
//! with `.itch.toml`). Signing runs when its secrets are in the environment
//! (Windows `signtool`/`osslsigncode` or Azure Trusted Signing, macOS
//! `codesign` + `notarytool`) and says it skipped otherwise. `--smoke`
//! copies the result outside the repository and runs it for 30 frames.
//!
//! A package builds on its own OS (CI has a runner for each); Windows also
//! builds on Linux where `x86_64-w64-mingw32-gcc` is installed.

pub mod icon;
pub mod winres;
pub mod zip;

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use serde_json::json;

use crate::project::Project;

const USAGE: &str = "xtask: usage:\n  \
    cargo xtask package pc [--target linux|windows|macos] [--steam] [--itch] [--out <dir>] \
    [--smoke] [--json]\n  \
    cargo xtask package 3ds --cia [--selftest | --no-build] [--out <dir>] [--json]\n  \
    cargo xtask package config [--json]\n  \
    cargo xtask package config set <key>=<value>.. (pc.icon, pc.identifier, steam.app_id, ..)";

/// The AppImage runtime (AppImage/type2-runtime, static, FUSE 3 or
/// extract-and-run), pinned by release and checksum.
const APPIMAGE_RUNTIME_URL: &str =
    "https://github.com/AppImage/type2-runtime/releases/download/20251108/runtime-x86_64";
const APPIMAGE_RUNTIME_SHA256: &str =
    "2fca8b443c92510f1483a883f60061ad09b46b978b2631c807cd873a47ec260d";

/// The `dist` profile, defined on the command line so a project needs no
/// change: release, fat LTO, one codegen unit, debug info stripped (symbol
/// names kept: crash reports name their functions), unwinding panics (the
/// crash hook writes its report on the way out).
const DIST_PROFILE: [&str; 5] = [
    "profile.dist.inherits=\"release\"",
    "profile.dist.lto=\"fat\"",
    "profile.dist.codegen-units=1",
    "profile.dist.strip=\"debuginfo\"",
    "profile.dist.debug=false",
];

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Os {
    Linux,
    Windows,
    Macos,
}

impl Os {
    pub fn parse(s: &str) -> Option<Os> {
        match s {
            "linux" => Some(Os::Linux),
            "windows" | "win" => Some(Os::Windows),
            "macos" | "mac" | "osx" => Some(Os::Macos),
            _ => None,
        }
    }

    pub fn host() -> Os {
        match std::env::consts::OS {
            "windows" => Os::Windows,
            "macos" => Os::Macos,
            _ => Os::Linux,
        }
    }

    pub fn key(self) -> &'static str {
        match self {
            Os::Linux => "linux",
            Os::Windows => "windows",
            Os::Macos => "macos",
        }
    }

    fn exe(self, bin: &str) -> String {
        match self {
            Os::Windows => format!("{bin}.exe"),
            _ => bin.to_string(),
        }
    }
}

/// Everything `package pc` decided before building.
#[derive(Clone, Debug)]
pub struct Plan {
    pub os: Os,
    /// `--target <triple>` (None: the host's).
    pub triple: Option<String>,
    pub name: String,
    pub title: String,
    pub bin: String,
    pub version: String,
    pub commit: String,
    pub date: String,
    pub features: Vec<String>,
    pub out: PathBuf,
    pub steam: bool,
    pub arch: &'static str,
}

impl Plan {
    /// `<name>-<version>-<os>-<arch>`.
    pub fn stem(&self) -> String {
        format!(
            "{}-{}-{}-{}",
            self.name,
            self.version,
            self.os.key(),
            self.arch
        )
    }
}

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let mut args: Vec<String> = args.iter().map(|s| s.to_string()).collect();
    let json = crate::web::take_flag(&mut args, "--json");
    let result = (|| -> Result<serde_json::Value, String> {
        let out = crate::web::take_value(&mut args, "--out")?.map(PathBuf::from);
        let target = crate::web::take_value(&mut args, "--target")?;
        let steam = crate::web::take_flag(&mut args, "--steam");
        let itch = crate::web::take_flag(&mut args, "--itch");
        let smoke = crate::web::take_flag(&mut args, "--smoke");
        let cia = crate::web::take_flag(&mut args, "--cia");
        let selftest = crate::web::take_flag(&mut args, "--selftest");
        let no_build = crate::web::take_flag(&mut args, "--no-build");
        let rest: Vec<&str> = args.iter().map(String::as_str).collect();
        match rest.as_slice() {
            ["pc"] => {
                let os = match &target {
                    Some(t) => {
                        Os::parse(t).ok_or(format!("--target {t}: linux, windows or macos"))?
                    }
                    None => Os::host(),
                };
                pc(project, os, out, steam, itch, smoke, json)
            }
            ["3ds"] if cia => crate::package::cia(project, out, selftest, no_build, json),
            ["config"] => config_show(project),
            ["config", "set", pairs @ ..] if !pairs.is_empty() => config_set(project, pairs),
            ["3ds"] => Err("package 3ds: pass --cia (the .3dsx is `cargo xtask build 3ds`)".into()),
            _ => Err(USAGE.into()),
        }
    })();
    match result {
        Ok(v) => {
            if json && !v.is_null() {
                println!("{}", serde_json::to_string_pretty(&v).unwrap());
            }
            ExitCode::SUCCESS
        }
        Err(e) => {
            if json {
                println!("{}", json!({ "error": e }));
            } else {
                eprintln!("{e}");
            }
            ExitCode::FAILURE
        }
    }
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

fn say(json: bool, line: &str) {
    if json {
        eprintln!("{line}");
    } else {
        println!("{line}");
    }
}

/// The PC crate as cargo sees it: version, bin name, features, and the
/// target directory.
struct Crate {
    version: String,
    bin: String,
    features: Vec<String>,
    target_dir: PathBuf,
}

fn metadata(project: &Project) -> Result<Crate, String> {
    let out = Command::new(cargo())
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(&project.root)
        .output()
        .map_err(|e| format!("cargo metadata: {e}"))?;
    if !out.status.success() {
        return Err(format!(
            "cargo metadata: {}",
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let v: serde_json::Value =
        serde_json::from_slice(&out.stdout).map_err(|e| format!("cargo metadata: {e}"))?;
    let pkg = v["packages"]
        .as_array()
        .and_then(|ps| ps.iter().find(|p| p["name"] == project.pc_package.as_str()))
        .ok_or_else(|| {
            format!(
                "no crate `{}` in {} ([pc] package)",
                project.pc_package,
                project.root.display()
            )
        })?;
    let bin = pkg["targets"]
        .as_array()
        .and_then(|ts| {
            ts.iter().find(|t| {
                t["kind"]
                    .as_array()
                    .is_some_and(|k| k.iter().any(|k| k == "bin"))
            })
        })
        .and_then(|t| t["name"].as_str())
        .ok_or_else(|| format!("`{}` has no binary", project.pc_package))?
        .to_string();
    let features = pkg["features"]
        .as_object()
        .map(|f| f.keys().cloned().collect())
        .unwrap_or_default();
    Ok(Crate {
        version: pkg["version"].as_str().unwrap_or("0.0.0").to_string(),
        bin,
        features,
        target_dir: PathBuf::from(v["target_directory"].as_str().unwrap_or("target")),
    })
}

/// `git rev-parse --short=10 HEAD`, `-dirty` with uncommitted changes;
/// `TRINO_GIT_COMMIT` wins; `unknown` outside git.
pub fn commit(root: &Path) -> String {
    if let Ok(c) = std::env::var("TRINO_GIT_COMMIT") {
        return c;
    }
    let git = |args: &[&str]| {
        Command::new("git")
            .args(args)
            .current_dir(root)
            .output()
            .ok()
            .filter(|o| o.status.success())
            .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
    };
    match git(&["rev-parse", "--short=10", "HEAD"]) {
        Some(c) => {
            let dirty = git(&["status", "--porcelain", "--untracked-files=no"])
                .is_some_and(|s| !s.is_empty());
            if dirty { format!("{c}-dirty") } else { c }
        }
        None => "unknown".into(),
    }
}

/// Today (UTC) as `YYYY-MM-DD`, or `SOURCE_DATE_EPOCH`'s day
/// (reproducible builds); `TRINO_BUILD_DATE` wins.
pub fn build_date() -> String {
    if let Ok(d) = std::env::var("TRINO_BUILD_DATE") {
        return d;
    }
    let secs = std::env::var("SOURCE_DATE_EPOCH")
        .ok()
        .and_then(|s| s.parse::<i64>().ok())
        .unwrap_or_else(|| {
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map_or(0, |d| d.as_secs() as i64)
        });
    civil(secs.div_euclid(86_400))
}

/// Days since 1970-01-01 as a date (Howard Hinnant's algorithm).
pub fn civil(days: i64) -> String {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z - era * 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    format!("{:04}-{m:02}-{d:02}", if m <= 2 { y + 1 } else { y })
}

fn have(tool: &str) -> bool {
    Command::new(tool)
        .arg("--version")
        .output()
        .is_ok_and(|o| o.status.success() || !o.stderr.is_empty() || !o.stdout.is_empty())
}

/// The triple to build for `os` from this host, or why it can't.
fn triple_for(os: Os) -> Result<Option<String>, String> {
    let host = Os::host();
    if os == host {
        return Ok(None);
    }
    if os == Os::Windows && host == Os::Linux && have("x86_64-w64-mingw32-gcc") {
        return Ok(Some("x86_64-pc-windows-gnu".into()));
    }
    Err(format!(
        "package pc --target {}: build it on {} (CI's `package` job has a runner per OS){}",
        os.key(),
        match os {
            Os::Windows => "Windows",
            Os::Macos => "a Mac",
            Os::Linux => "Linux",
        },
        if os == Os::Windows {
            ", or install mingw-w64 (x86_64-w64-mingw32-gcc) to cross-build from Linux"
        } else {
            ""
        }
    ))
}

fn arch_of(triple: Option<&str>) -> &'static str {
    match triple {
        Some(t) if t.starts_with("aarch64") => "arm64",
        Some(_) => "x86_64",
        None => match std::env::consts::ARCH {
            "aarch64" => "arm64",
            _ => "x86_64",
        },
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), String> {
    std::fs::create_dir_all(to).map_err(|e| format!("{}: {e}", to.display()))?;
    for e in std::fs::read_dir(from).map_err(|e| format!("{}: {e}", from.display()))? {
        let e = e.map_err(|e| e.to_string())?;
        let (src, dst) = (e.path(), to.join(e.file_name()));
        if src.is_dir() {
            copy_dir(&src, &dst)?;
        } else {
            std::fs::copy(&src, &dst).map_err(|e| format!("{}: {e}", src.display()))?;
        }
    }
    Ok(())
}

fn write(path: &Path, bytes: impl AsRef<[u8]>) -> Result<(), String> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d).map_err(|e| format!("{}: {e}", d.display()))?;
    }
    std::fs::write(path, bytes).map_err(|e| format!("{}: {e}", path.display()))
}

#[cfg(unix)]
fn make_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755))
        .map_err(|e| format!("{}: {e}", p.display()))
}

#[cfg(not(unix))]
fn make_executable(_: &Path) -> Result<(), String> {
    Ok(())
}

/// The marker the runner reads (`trino_runner_pc::package`).
pub fn marker(project: &Project, plan: &Plan, assets: &str) -> String {
    let mut out = String::from("# Written by `cargo xtask package` (Trino, issue #61).\n");
    let mut kv = |k: &str, v: &str| {
        if !v.is_empty() {
            out.push_str(&format!("{k}={v}\n"));
        }
    };
    kv("name", &plan.name);
    kv("title", &plan.title);
    kv("version", &plan.version);
    kv("commit", &plan.commit);
    kv("date", &plan.date);
    kv("publisher", &project.ship.publisher);
    kv("identifier", &project.ship.identifier);
    kv("save_dir", &plan.title);
    kv("assets", assets);
    kv("profile", &project.ship.profile);
    if let Some(id) = project.steam_app_id {
        kv("steam_app_id", &id.to_string());
    }
    if let Some(u) = &project.ship.crash_upload {
        kv("crash_upload", u);
    }
    out
}

/// The project's icon, or the engine's default.
fn icon_of(project: &Project) -> Result<icon::Image, String> {
    match &project.ship.icon {
        Some(p) => {
            let img = icon::Image::load(p)?;
            if img.w != img.h {
                return Err(format!(
                    "[pc] icon {}: {}x{} — use a square PNG (256 x 256 or bigger)",
                    p.display(),
                    img.w,
                    img.h
                ));
            }
            Ok(img)
        }
        None => Ok(icon::default_icon()),
    }
}

/// `package pc`.
fn pc(
    project: &Project,
    os: Os,
    out: Option<PathBuf>,
    steam: bool,
    itch: bool,
    smoke: bool,
    json: bool,
) -> Result<serde_json::Value, String> {
    if project.is_engine {
        return Err(
            "package pc packages a project (`--project <dir>`, or run it from one): the \
             engine's apps/pc is a development host. `cargo xtask new <name> --standalone <dir>` \
             makes one."
                .into(),
        );
    }
    let triple = triple_for(os)?;
    let krate = metadata(project)?;
    let mut features = match &project.ship.features {
        Some(f) => f.clone(),
        None => ["gamepad"]
            .iter()
            .filter(|f| krate.features.iter().any(|k| k == *f))
            .map(|f| f.to_string())
            .collect(),
    };
    if steam {
        if !krate.features.iter().any(|f| f == "steam") {
            return Err(format!(
                "package pc --steam: `{}` has no `steam` feature; add \
                 `steam = [\"trino-runner-pc/steam\"]` to its [features]",
                project.pc_package
            ));
        }
        if project.steam_app_id.is_none() {
            return Err("package pc --steam: set [steam] app_id in trino.toml".into());
        }
        features.push("steam".into());
    }
    let plan = Plan {
        os,
        arch: arch_of(triple.as_deref()),
        triple,
        name: project.name.clone(),
        title: project.title.clone(),
        bin: krate.bin.clone(),
        version: project
            .ship
            .version
            .clone()
            .unwrap_or_else(|| krate.version.clone()),
        commit: commit(&project.root),
        date: build_date(),
        features,
        out: out.unwrap_or_else(|| project.root.join("dist")),
        steam,
    };
    let work = krate.target_dir.join("package").join(os.key());
    let _ = std::fs::remove_dir_all(&work);
    std::fs::create_dir_all(&work).map_err(|e| format!("{}: {e}", work.display()))?;
    std::fs::create_dir_all(&plan.out).map_err(|e| format!("{}: {e}", plan.out.display()))?;
    say(
        json,
        &format!(
            "[package] {} {} ({}) for {}: features [{}]",
            plan.name,
            plan.version,
            plan.commit,
            os.key(),
            plan.features.join(", ")
        ),
    );

    // 1. The executable.
    let icon = icon_of(project)?;
    let exe = build(project, &plan, &krate, &icon, &work, json)?;

    // 2. The game folder: executable, marker, baked assets (+ Steam's lib).
    let folder = work.join("game");
    let (game_dir, assets_rel) = match os {
        Os::Macos => (
            folder.join(format!("{}.app/Contents/Resources", plan.title)),
            "assets/pc",
        ),
        _ => (folder.join(&plan.name), "assets/pc"),
    };
    let exe_dir = match os {
        Os::Macos => folder.join(format!("{}.app/Contents/MacOS", plan.title)),
        _ => game_dir.clone(),
    };
    std::fs::create_dir_all(&exe_dir).map_err(|e| e.to_string())?;
    let exe_name = os.exe(&plan.bin);
    std::fs::copy(&exe, exe_dir.join(&exe_name)).map_err(|e| format!("{}: {e}", exe.display()))?;
    make_executable(&exe_dir.join(&exe_name))?;
    trino_asset_pipeline::bake_all(
        &project.root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &game_dir.join(assets_rel),
    )
    .map_err(|e| format!("asset bake: {e}"))?;
    write(
        &game_dir.join("trino-package.txt"),
        marker(project, &plan, assets_rel),
    )?;
    write(&game_dir.join("icon.png"), icon.resized(256).png())?;
    if steam {
        copy_steam_lib(&krate, &plan, &exe_dir)?;
    }

    // 3. The OS's shape.
    let mut artifacts = Vec::new();
    let signed = match os {
        Os::Windows => {
            let s = sign_windows(&exe_dir.join(&exe_name), json)?;
            let zip = plan.out.join(format!("{}.zip", plan.stem()));
            zip::zip_dir(&game_dir, &plan.name, &zip)?;
            artifacts.push(zip);
            s
        }
        Os::Macos => {
            let app = folder.join(format!("{}.app", plan.title));
            write_bundle(project, &plan, &icon, &app)?;
            let s = sign_macos(&app, &plan, json)?;
            let zip = plan.out.join(format!("{}.zip", plan.stem()));
            zip::zip_dir(&folder, "", &zip)?;
            artifacts.push(zip);
            if Os::host() == Os::Macos {
                artifacts.push(dmg(&plan, &folder)?);
            }
            s
        }
        Os::Linux => {
            artifacts.push(appimage(project, &plan, &icon, &game_dir, &work)?);
            let zip = plan.out.join(format!("{}.zip", plan.stem()));
            zip::zip_dir(&game_dir, &plan.name, &zip)?;
            artifacts.push(zip);
            "none (Linux packages are not signed)".to_string()
        }
    };
    let content = match os {
        Os::Macos => folder.clone(),
        _ => game_dir.clone(),
    };
    let mut extra = json!({});
    if steam {
        extra["steam"] = json!(steam_layout(project, &plan, &content)?);
    }
    if itch {
        extra["itch"] = json!(itch_layout(project, &plan, &content)?);
    }
    let smoke_line = if smoke {
        Some(smoke_run(&plan, &artifacts, &content, json)?)
    } else {
        None
    };
    for a in &artifacts {
        let size = std::fs::metadata(a).map_or(0, |m| m.len());
        say(
            json,
            &format!("[package] {} ({:.1} MB)", a.display(), size as f64 / 1e6),
        );
    }
    say(json, &format!("[package] signing: {signed}"));
    Ok(json!({
        "os": os.key(),
        "name": plan.name,
        "version": plan.version,
        "commit": plan.commit,
        "date": plan.date,
        "features": plan.features,
        "artifacts": artifacts,
        "folder": content,
        "signed": signed,
        "smoke": smoke_line,
        "extra": extra,
    }))
}

/// `cargo rustc` the PC crate with the `dist` profile, the stamp and, on
/// Windows, the resources and the GUI subsystem. Returns the executable.
fn build(
    project: &Project,
    plan: &Plan,
    krate: &Crate,
    icon: &icon::Image,
    work: &Path,
    json: bool,
) -> Result<PathBuf, String> {
    let mut cmd = Command::new(cargo());
    cmd.current_dir(&project.root)
        .args(["rustc", "-p", &project.pc_package, "--bin", &plan.bin])
        .args(["--profile", "dist", "--no-default-features"]);
    for c in DIST_PROFILE {
        cmd.args(["--config", c]);
    }
    if !plan.features.is_empty() {
        cmd.args(["--features", &plan.features.join(",")]);
    }
    if let Some(t) = &plan.triple {
        cmd.args(["--target", t]);
    }
    cmd.env("TRINO_GAME_VERSION", &plan.version)
        .env("TRINO_GIT_COMMIT", &plan.commit)
        .env("TRINO_BUILD_DATE", &plan.date);
    let mut link: Vec<String> = Vec::new();
    if plan.os == Os::Windows {
        let res = work.join(format!("{}.res", plan.bin));
        let info = winres::VersionInfo {
            version: plan.version.clone(),
            company: project.ship.publisher.clone(),
            product: plan.title.clone(),
            description: if project.description.is_empty() {
                plan.title.clone()
            } else {
                project.description.clone()
            },
            copyright: format!(
                "© {} {}",
                &plan.date[..4.min(plan.date.len())],
                project.ship.publisher
            ),
            filename: format!("{}.exe", plan.bin),
        };
        write(&res, winres::res(&icon::ico_images(icon), &info))?;
        write(
            &work.join(format!("{}.ico", plan.name)),
            icon::ico(&icon::ico_images(icon)),
        )?;
        let gnu = plan.triple.as_deref().is_some_and(|t| t.ends_with("-gnu"))
            || (plan.triple.is_none() && cfg!(target_env = "gnu"));
        if gnu {
            let obj = work.join(format!("{}.res.o", plan.bin));
            let windres = if Os::host() == Os::Windows {
                "windres"
            } else {
                "x86_64-w64-mingw32-windres"
            };
            let ok = Command::new(windres)
                .args(["--input-format=res", "--output-format=coff", "-i"])
                .arg(&res)
                .arg("-o")
                .arg(&obj)
                .status()
                .is_ok_and(|s| s.success());
            if ok {
                link.push(obj.display().to_string());
            } else {
                say(
                    json,
                    &format!("[package] {windres} failed: the .exe has no icon or version info"),
                );
            }
            link.push("-mwindows".into());
        } else {
            link.push(res.display().to_string());
            link.push("/SUBSYSTEM:WINDOWS".into());
            link.push("/ENTRY:mainCRTStartup".into());
        }
    }
    if plan.os == Os::Linux && plan.steam {
        // libsteam_api.so sits beside the executable.
        link.push("-Wl,-rpath,$ORIGIN".into());
    }
    if !link.is_empty() {
        cmd.arg("--");
        for l in &link {
            cmd.args(["-C", &format!("link-arg={l}")]);
        }
    }
    say(
        json,
        &format!(
            "[package] cargo rustc -p {} --profile dist",
            project.pc_package
        ),
    );
    let status = if json {
        cmd.stdout(std::process::Stdio::null())
            .status()
            .map_err(|e| format!("cargo: {e}"))?
    } else {
        cmd.status().map_err(|e| format!("cargo: {e}"))?
    };
    if !status.success() {
        return Err(format!("package: building `{}` failed", project.pc_package));
    }
    let mut dir = krate.target_dir.clone();
    if let Some(t) = &plan.triple {
        dir = dir.join(t);
    }
    let exe = dir.join("dist").join(plan.os.exe(&plan.bin));
    if !exe.is_file() {
        return Err(format!("package: {} was not produced", exe.display()));
    }
    Ok(exe)
}

/// Steam's runtime library from the `steamworks-sys` build, beside the
/// executable.
fn copy_steam_lib(krate: &Crate, plan: &Plan, exe_dir: &Path) -> Result<(), String> {
    let name = match plan.os {
        Os::Windows => "steam_api64.dll",
        Os::Macos => "libsteam_api.dylib",
        Os::Linux => "libsteam_api.so",
    };
    let mut build = krate.target_dir.clone();
    if let Some(t) = &plan.triple {
        build = build.join(t);
    }
    let build = build.join("dist/build");
    let mut found: Option<(std::time::SystemTime, PathBuf)> = None;
    for e in std::fs::read_dir(&build).map_err(|e| format!("{}: {e}", build.display()))? {
        let p = e.map_err(|e| e.to_string())?.path();
        let lib = p.join("out").join(name);
        if p.file_name()
            .is_some_and(|n| n.to_string_lossy().starts_with("steamworks-sys-"))
            && lib.is_file()
        {
            let t = std::fs::metadata(&lib)
                .and_then(|m| m.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            if found.as_ref().is_none_or(|(old, _)| t > *old) {
                found = Some((t, lib));
            }
        }
    }
    let (_, lib) =
        found.ok_or_else(|| format!("{name}: not in {} (steamworks-sys)", build.display()))?;
    std::fs::copy(&lib, exe_dir.join(name)).map_err(|e| format!("{}: {e}", lib.display()))?;
    Ok(())
}

fn xml(s: &str) -> String {
    s.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
}

/// `Info.plist` for the bundle.
pub fn info_plist(project: &Project, plan: &Plan) -> String {
    let kv = |k: &str, v: &str| format!("\t<key>{k}</key>\n\t<string>{}</string>\n", xml(v));
    let mut s = String::from(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<!DOCTYPE plist PUBLIC \"-//Apple//DTD PLIST 1.0//EN\" \
         \"http://www.apple.com/DTDs/PropertyList-1.0.dtd\">\n<plist version=\"1.0\">\n<dict>\n",
    );
    s += &kv("CFBundleName", &plan.title);
    s += &kv("CFBundleDisplayName", &plan.title);
    s += &kv("CFBundleIdentifier", &project.ship.identifier);
    s += &kv("CFBundleVersion", &plan.version);
    s += &kv("CFBundleShortVersionString", &plan.version);
    s += &kv("CFBundleExecutable", &plan.bin);
    s += &kv("CFBundleIconFile", &format!("{}.icns", plan.name));
    s += &kv("CFBundlePackageType", "APPL");
    s += &kv("CFBundleInfoDictionaryVersion", "6.0");
    s += &kv("LSMinimumSystemVersion", "11.0");
    s += &kv("LSApplicationCategoryType", "public.app-category.games");
    s += &kv(
        "NSHumanReadableCopyright",
        &format!(
            "© {} {}",
            &plan.date[..4.min(plan.date.len())],
            project.ship.publisher
        ),
    );
    s += "\t<key>NSHighResolutionCapable</key>\n\t<true/>\n</dict>\n</plist>\n";
    s
}

fn write_bundle(
    project: &Project,
    plan: &Plan,
    icon: &icon::Image,
    app: &Path,
) -> Result<(), String> {
    write(&app.join("Contents/Info.plist"), info_plist(project, plan))?;
    write(&app.join("Contents/PkgInfo"), "APPL????")?;
    write(
        &app.join(format!("Contents/Resources/{}.icns", plan.name)),
        icon::icns(icon),
    )
}

/// The `.dmg` (`hdiutil`), tried twice: hosted Macs fail it now and then
/// with "Resource busy".
fn dmg(plan: &Plan, folder: &Path) -> Result<PathBuf, String> {
    let out = plan.out.join(format!("{}.dmg", plan.stem()));
    for attempt in 0..2 {
        let _ = std::fs::remove_file(&out);
        let ok = Command::new("hdiutil")
            .args(["create", "-volname", &plan.title, "-srcfolder"])
            .arg(folder)
            .args(["-ov", "-format", "UDZO"])
            .arg(&out)
            .stdout(std::io::stderr())
            .status()
            .is_ok_and(|s| s.success());
        if ok {
            return Ok(out);
        }
        if attempt == 0 {
            std::thread::sleep(std::time::Duration::from_secs(5));
        }
    }
    Err("hdiutil create failed twice".into())
}

/// The `.desktop` entry an AppImage carries.
pub fn desktop_entry(project: &Project, plan: &Plan) -> String {
    format!(
        "[Desktop Entry]\nType=Application\nName={}\nComment={}\nExec={}\nIcon={}\nCategories=Game;\n\
         Terminal=false\nX-AppImage-Version={}\n",
        plan.title,
        if project.description.is_empty() {
            &plan.title
        } else {
            &project.description
        },
        plan.bin,
        plan.name,
        plan.version
    )
}

/// The AppImage runtime, downloaded once into the engine's `target/tools`
/// and checked against its pinned SHA-256.
fn appimage_runtime(engine: &Path) -> Result<PathBuf, String> {
    let dir = engine.join("target/tools");
    let path = dir.join("appimage-runtime-x86_64-20251108");
    if !path.is_file() {
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let partial = path.with_extension("part");
        let ok = Command::new("curl")
            .args(["-fsSL", "-o"])
            .arg(&partial)
            .arg(APPIMAGE_RUNTIME_URL)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return Err(format!("download {APPIMAGE_RUNTIME_URL} failed (curl)"));
        }
        std::fs::rename(&partial, &path).map_err(|e| e.to_string())?;
    }
    let out = Command::new("sha256sum")
        .arg(&path)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    let sum = String::from_utf8_lossy(&out.stdout);
    if !sum.starts_with(APPIMAGE_RUNTIME_SHA256) {
        let _ = std::fs::remove_file(&path);
        return Err(format!("{}: checksum mismatch ({sum})", path.display()));
    }
    Ok(path)
}

fn appimage(
    project: &Project,
    plan: &Plan,
    icon: &icon::Image,
    game_dir: &Path,
    work: &Path,
) -> Result<PathBuf, String> {
    if !have("mksquashfs") && Command::new("mksquashfs").arg("-version").output().is_err() {
        return Err(
            "package pc --target linux: mksquashfs is missing (apt install squashfs-tools)".into(),
        );
    }
    let app = work.join("AppDir");
    let lib = app.join("usr/lib").join(&plan.name);
    copy_dir(game_dir, &lib)?;
    let run = format!(
        "#!/bin/sh\n# {title} (Trino): the game lives in usr/lib/{name}.\nHERE=\"$(dirname \"$(readlink -f \"$0\")\")\"\n\
         export LD_LIBRARY_PATH=\"$HERE/usr/lib/{name}${{LD_LIBRARY_PATH:+:$LD_LIBRARY_PATH}}\"\n\
         exec \"$HERE/usr/lib/{name}/{bin}\" \"$@\"\n",
        title = plan.title,
        name = plan.name,
        bin = plan.bin
    );
    write(&app.join("AppRun"), run)?;
    make_executable(&app.join("AppRun"))?;
    let desktop = desktop_entry(project, plan);
    write(&app.join(format!("{}.desktop", plan.name)), &desktop)?;
    write(
        &app.join(format!("usr/share/applications/{}.desktop", plan.name)),
        &desktop,
    )?;
    let png = icon.resized(256).png();
    write(&app.join(format!("{}.png", plan.name)), &png)?;
    write(&app.join(".DirIcon"), &png)?;
    write(
        &app.join(format!(
            "usr/share/icons/hicolor/256x256/apps/{}.png",
            plan.name
        )),
        &png,
    )?;
    let squash = work.join("app.squashfs");
    let _ = std::fs::remove_file(&squash);
    let ok = Command::new("mksquashfs")
        .arg(&app)
        .arg(&squash)
        .args([
            "-root-owned",
            "-noappend",
            "-comp",
            "zstd",
            "-no-progress",
            "-quiet",
        ])
        .stdout(std::io::stderr())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err("mksquashfs failed".into());
    }
    let runtime = appimage_runtime(&project.engine)?;
    let out = plan.out.join(format!("{}.AppImage", plan.stem()));
    let mut bytes = std::fs::read(&runtime).map_err(|e| e.to_string())?;
    bytes.extend(std::fs::read(&squash).map_err(|e| e.to_string())?);
    write(&out, bytes)?;
    make_executable(&out)?;
    Ok(out)
}

/// The SteamPipe layout: `steam/content/<os>` and the build scripts.
fn steam_layout(
    project: &Project,
    plan: &Plan,
    content: &Path,
) -> Result<serde_json::Value, String> {
    let app = project.steam_app_id.unwrap_or(0);
    let root = plan.out.join("steam");
    let dest = root.join("content").join(plan.os.key());
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir(content, &dest)?;
    let depots: Vec<trino_scene::steam::Depot> = ["windows", "linux", "macos"]
        .iter()
        .filter_map(|os| {
            project
                .steam_depots
                .get(*os)
                .map(|&id| trino_scene::steam::Depot {
                    id,
                    os: match *os {
                        "windows" => "windows",
                        "linux" => "linux",
                        _ => "macos",
                    },
                })
        })
        .collect();
    let scripts = root.join("scripts");
    write(
        &scripts.join(format!("app_build_{app}.vdf")),
        trino_scene::steam::app_build_vdf(
            app,
            &format!("{} {}", plan.version, plan.commit),
            &depots,
        ),
    )?;
    for d in &depots {
        // Paths in a depot script are relative to ContentRoot (`../content`).
        write(
            &scripts.join(format!("depot_build_{}.vdf", d.id)),
            trino_scene::steam::depot_build_vdf(d),
        )?;
    }
    let launch = match plan.os {
        Os::Windows => format!("{}.exe", plan.bin),
        Os::Macos => format!("{}.app", plan.title),
        Os::Linux => plan.bin.clone(),
    };
    let readme = format!(
        "SteamPipe layout for {title} {version} ({commit}).\n\n\
         Upload: steamcmd +login <builder> +run_app_build \"$PWD/scripts/app_build_{app}.vdf\" +quit\n\
         (run it from this folder; the scripts read content/<os>).\n\n\
         Launch option for {os}: {launch}\n\n\
         Steam Auto-Cloud (Steamworks > Application > Steam Cloud): one root per OS, all with\n\
         subdirectory \"{save}\" and patterns *.sav and *.achievements:\n\
         - Windows: WinAppDataRoaming\n- macOS: MacAppSupport\n- Linux: LinuxXdgDataHome\n\
         (the folder trino-runner-pc saves to in a package; display, quality and input\n\
         settings stay per machine, crash reports are never synced).\n",
        title = plan.title,
        version = plan.version,
        commit = plan.commit,
        os = plan.os.key(),
        save = trino_package_dir(&plan.title),
    );
    write(&root.join("README.txt"), readme)?;
    Ok(
        json!({ "root": root, "content": dest, "app_id": app, "depots": depots.iter().map(|d| (d.os, d.id)).collect::<Vec<_>>() }),
    )
}

/// The user-data folder name the runner uses (`package::sanitize`).
fn trino_package_dir(title: &str) -> String {
    title
        .chars()
        .map(|c| match c {
            '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|' => '_',
            c => c,
        })
        .collect::<String>()
        .trim()
        .to_string()
}

/// butler's layout: `itch/<os>` with `.itch.toml`.
fn itch_layout(
    project: &Project,
    plan: &Plan,
    content: &Path,
) -> Result<serde_json::Value, String> {
    let dest = plan.out.join("itch").join(plan.os.key());
    let _ = std::fs::remove_dir_all(&dest);
    copy_dir(content, &dest)?;
    let path = match plan.os {
        Os::Windows => format!("{}/{}.exe", plan.name, plan.bin),
        Os::Macos => format!("{}.app", plan.title),
        Os::Linux => format!("{}/{}", plan.name, plan.bin),
    };
    // The content keeps the game folder at its root for Windows and Linux.
    if plan.os != Os::Macos {
        let inner = dest.join(&plan.name);
        let tmp = plan.out.join("itch").join(format!(".{}", plan.os.key()));
        let _ = std::fs::remove_dir_all(&tmp);
        std::fs::rename(&dest, &tmp).map_err(|e| e.to_string())?;
        std::fs::create_dir_all(&dest).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &inner).map_err(|e| e.to_string())?;
    }
    write(
        &dest.join(".itch.toml"),
        format!("[[actions]]\nname = \"play\"\npath = \"{path}\"\n"),
    )?;
    let target = project
        .itch_target
        .clone()
        .unwrap_or_else(|| "<user>/<game>".into());
    let push = format!(
        "butler push {} {target}:{} --userversion {}",
        dest.display(),
        plan.os.key(),
        plan.version
    );
    Ok(json!({ "dir": dest, "push": push }))
}

/// Sign a Windows executable when the environment has a certificate:
/// `TRINO_WINDOWS_SIGN_CERT` (.pfx) + `TRINO_WINDOWS_SIGN_PASSWORD`
/// (signtool on Windows, osslsigncode elsewhere), or Azure Trusted Signing
/// (`TRINO_AZURE_SIGNING_DLIB` + `TRINO_AZURE_SIGNING_METADATA`, signtool).
fn sign_windows(exe: &Path, json: bool) -> Result<String, String> {
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let ts = env("TRINO_WINDOWS_TIMESTAMP_URL")
        .unwrap_or_else(|| "http://timestamp.digicert.com".into());
    if let (Some(dlib), Some(meta)) = (
        env("TRINO_AZURE_SIGNING_DLIB"),
        env("TRINO_AZURE_SIGNING_METADATA"),
    ) {
        let ok = Command::new("signtool")
            .args([
                "sign", "/v", "/fd", "SHA256", "/tr", &ts, "/td", "SHA256", "/dlib", &dlib,
                "/dmdf", &meta,
            ])
            .arg(exe)
            .stdout(std::io::stderr())
            .status()
            .is_ok_and(|s| s.success());
        return if ok {
            Ok("Azure Trusted Signing".into())
        } else {
            Err("signtool (Azure Trusted Signing) failed".into())
        };
    }
    if let Some(cert) = env("TRINO_WINDOWS_SIGN_CERT") {
        let pass = env("TRINO_WINDOWS_SIGN_PASSWORD").unwrap_or_default();
        let ok = if Os::host() == Os::Windows {
            Command::new("signtool")
                .args([
                    "sign", "/fd", "SHA256", "/tr", &ts, "/td", "SHA256", "/f", &cert, "/p", &pass,
                ])
                .arg(exe)
                .stdout(std::io::stderr())
                .status()
                .is_ok_and(|s| s.success())
        } else {
            let signed = exe.with_extension("signed.exe");
            let ok = Command::new("osslsigncode")
                .args([
                    "sign", "-pkcs12", &cert, "-pass", &pass, "-h", "sha256", "-ts", &ts, "-in",
                ])
                .arg(exe)
                .arg("-out")
                .arg(&signed)
                .stdout(std::io::stderr())
                .status()
                .is_ok_and(|s| s.success());
            ok && std::fs::rename(&signed, exe).is_ok()
        };
        return if ok {
            Ok("Authenticode (certificate)".into())
        } else {
            Err("Windows signing failed".into())
        };
    }
    say(
        json,
        "[package] signing skipped: set TRINO_WINDOWS_SIGN_CERT (+ TRINO_WINDOWS_SIGN_PASSWORD), or \
         TRINO_AZURE_SIGNING_DLIB + TRINO_AZURE_SIGNING_METADATA (CI secrets, never in the repo)",
    );
    Ok("none".into())
}

/// Sign (and notarize) a macOS bundle: `TRINO_MACOS_SIGN_IDENTITY` (a
/// Developer ID Application identity in the keychain), then notarization
/// with `TRINO_NOTARY_PROFILE` (a `notarytool store-credentials` profile)
/// or `TRINO_NOTARY_APPLE_ID` + `TRINO_NOTARY_PASSWORD` +
/// `TRINO_NOTARY_TEAM_ID`. Without an identity the bundle is signed ad hoc
/// (it runs on the Mac that built it and where Gatekeeper is told to).
fn sign_macos(app: &Path, plan: &Plan, json: bool) -> Result<String, String> {
    if Os::host() != Os::Macos {
        return Ok("none (codesign runs on macOS)".into());
    }
    let env = |k: &str| std::env::var(k).ok().filter(|v| !v.is_empty());
    let Some(identity) = env("TRINO_MACOS_SIGN_IDENTITY") else {
        let ok = Command::new("codesign")
            .args(["--force", "--deep", "--sign", "-"])
            .arg(app)
            .stdout(std::io::stderr())
            .status()
            .is_ok_and(|s| s.success());
        say(
            json,
            "[package] signing: ad hoc (set TRINO_MACOS_SIGN_IDENTITY and TRINO_NOTARY_* to sign and notarize)",
        );
        return if ok {
            Ok("ad hoc".into())
        } else {
            Err("codesign (ad hoc) failed".into())
        };
    };
    let ok = Command::new("codesign")
        .args([
            "--force",
            "--deep",
            "--options",
            "runtime",
            "--timestamp",
            "--sign",
            &identity,
        ])
        .arg(app)
        .stdout(std::io::stderr())
        .status()
        .is_ok_and(|s| s.success());
    if !ok {
        return Err("codesign failed".into());
    }
    let creds: Option<Vec<String>> = match (
        env("TRINO_NOTARY_PROFILE"),
        env("TRINO_NOTARY_APPLE_ID"),
        env("TRINO_NOTARY_PASSWORD"),
        env("TRINO_NOTARY_TEAM_ID"),
    ) {
        (Some(p), ..) => Some(vec!["--keychain-profile".into(), p]),
        (None, Some(id), Some(pw), Some(team)) => Some(vec![
            "--apple-id".into(),
            id,
            "--password".into(),
            pw,
            "--team-id".into(),
            team,
        ]),
        _ => None,
    };
    let Some(creds) = creds else {
        say(
            json,
            "[package] notarization skipped: set TRINO_NOTARY_PROFILE or TRINO_NOTARY_APPLE_ID/_PASSWORD/_TEAM_ID",
        );
        return Ok(format!("Developer ID ({identity}), not notarized"));
    };
    let zip = app.with_extension("notarize.zip");
    let ok = Command::new("ditto")
        .args(["-c", "-k", "--keepParent"])
        .arg(app)
        .arg(&zip)
        .stdout(std::io::stderr())
        .status()
        .is_ok_and(|s| s.success())
        && Command::new("xcrun")
            .args(["notarytool", "submit"])
            .arg(&zip)
            .args(&creds)
            .arg("--wait")
            .stdout(std::io::stderr())
            .status()
            .is_ok_and(|s| s.success())
        && Command::new("xcrun")
            .args(["stapler", "staple"])
            .arg(app)
            .stdout(std::io::stderr())
            .status()
            .is_ok_and(|s| s.success());
    let _ = std::fs::remove_file(&zip);
    let _ = plan;
    if ok {
        Ok(format!("Developer ID ({identity}), notarized"))
    } else {
        Err("notarization failed (xcrun notarytool / stapler)".into())
    }
}

/// Run the package from a folder outside the repository for 30 frames
/// (`TRINO_SMOKE_FRAMES`) with saves in a scratch folder — what a player's
/// first launch does, minus the window staying open — writing the last
/// frame to `<out>/<stem>-smoke.png`; then again with `TRINO_CRASH_AFTER=3`,
/// which must exit with a crash report in the save folder. Returns the
/// `TRINO_RUN_OK` line.
fn smoke_run(
    plan: &Plan,
    artifacts: &[PathBuf],
    content: &Path,
    json: bool,
) -> Result<String, String> {
    let base = std::env::temp_dir().join(format!("trino-package-smoke-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&base);
    std::fs::create_dir_all(&base).map_err(|e| e.to_string())?;
    let exe = match plan.os {
        Os::Linux => {
            let image = artifacts
                .iter()
                .find(|a| a.extension().is_some_and(|e| e == "AppImage"))
                .ok_or("no AppImage")?;
            let copy = base.join(image.file_name().unwrap());
            std::fs::copy(image, &copy).map_err(|e| e.to_string())?;
            make_executable(&copy)?;
            copy
        }
        Os::Windows => {
            let dir = base.join(&plan.name);
            copy_dir(content, &dir)?;
            dir.join(format!("{}.exe", plan.bin))
        }
        Os::Macos => {
            copy_dir(content, &base.join("app"))?;
            base.join("app")
                .join(format!("{}.app/Contents/MacOS/{}", plan.title, plan.bin))
        }
    };
    let save = base.join("save");
    let shot = std::path::absolute(plan.out.join(format!("{}-smoke.png", plan.stem())))
        .map_err(|e| e.to_string())?;
    let run = |extra: &[(&str, &str)]| -> Result<(bool, String), String> {
        let mut cmd = Command::new(&exe);
        // No FUSE on CI runners: the AppImage runtime extracts and runs.
        cmd.current_dir(&base)
            .env("APPIMAGE_EXTRACT_AND_RUN", "1")
            .env("TRINO_SMOKE_FRAMES", "30")
            .env("TRINO_SAVE_DIR", &save)
            .env("TRINO_WINDOWED", "1");
        for (k, v) in extra {
            cmd.env(k, v);
        }
        let out = cmd.output().map_err(|e| format!("smoke run: {e}"))?;
        let text = format!(
            "{}{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr)
        );
        Ok((out.status.success(), text))
    };
    say(
        json,
        "[package] smoke: running the package outside the repository",
    );
    let (ok, text) = run(&[("TRINO_SCREENSHOT", &shot.display().to_string())])?;
    let line = text
        .lines()
        .find(|l| l.starts_with("TRINO_RUN_OK"))
        .map(str::to_string);
    let line = match (ok, line) {
        (true, Some(l)) => l,
        _ => {
            let _ = std::fs::remove_dir_all(&base);
            return Err(format!("smoke run failed:\n{text}"));
        }
    };
    say(
        json,
        &format!("[package] smoke: {line} (frame in {})", shot.display()),
    );
    let (ok, text) = run(&[("TRINO_CRASH_AFTER", "3")])?;
    let reports: Vec<PathBuf> = std::fs::read_dir(save.join("crashes"))
        .map(|d| d.flatten().map(|e| e.path()).collect())
        .unwrap_or_default();
    let _ = std::fs::remove_dir_all(&base);
    if ok || reports.len() != 1 {
        return Err(format!(
            "crash check: expected a failing run and one report, got exit ok={ok}, {} report(s):\n{text}",
            reports.len()
        ));
    }
    say(
        json,
        "[package] smoke: a deliberate panic wrote its crash report in the save folder",
    );
    Ok(line)
}

/// `package config`: the packaging keys as `trino.toml` has them and
/// what `package pc` would use.
fn config_show(project: &Project) -> Result<serde_json::Value, String> {
    let ship = trino_asset_pipeline::ship::Ship::load(&project.root)?;
    let v = json!({
        "file": trino_asset_pipeline::ship::path(&project.root),
        "set": {
            "pc.icon": ship.icon, "pc.identifier": ship.identifier,
            "pc.publisher": ship.publisher, "pc.version": ship.version,
            "pc.features": ship.features, "pc.profile": ship.profile,
            "pc.crash_upload": ship.crash_upload, "steam.app_id": ship.steam_app_id,
            "itch.target": ship.itch_target, "project.title": ship.title,
        },
        "effective": {
            "identifier": project.ship.identifier, "publisher": project.ship.publisher,
            "profile": project.ship.profile, "icon": project.ship.icon,
            "steam_depots": project.steam_depots,
        },
        "keys": trino_asset_pipeline::ship::KEYS.iter().map(|(k, w)| json!({"key": k, "takes": w})).collect::<Vec<_>>(),
    });
    println!("{}", serde_json::to_string_pretty(&v).unwrap());
    Ok(json!(null))
}

/// `package config set k=v ..`: written in place (comments kept), then
/// read back as `package pc` will.
fn config_set(project: &Project, pairs: &[&str]) -> Result<serde_json::Value, String> {
    for pair in pairs {
        let (k, v) = pair.split_once('=').ok_or(format!(
            "`{pair}`: write key=value (an empty value removes it)"
        ))?;
        trino_asset_pipeline::ship::set_in(&project.root, k.trim(), v.trim())?;
        println!("{} = {}", k.trim(), v.trim());
    }
    if !project.is_engine {
        Project::load(&project.root)?;
    }
    Ok(json!(null))
}

/// `package 3ds --cia`: see [`cia`](crate::package::cia).
pub fn cia(
    project: &Project,
    out: Option<PathBuf>,
    selftest: bool,
    no_build: bool,
    json: bool,
) -> Result<serde_json::Value, String> {
    crate::n3ds_cia::cia(project, out, selftest, no_build, json)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_and_names() {
        assert_eq!(civil(0), "1970-01-01");
        assert_eq!(civil(20_384), "2025-10-23");
        assert_eq!(civil(11_016), "2000-02-29");
        assert_eq!(Os::parse("win"), Some(Os::Windows));
        assert_eq!(Os::Windows.exe("g"), "g.exe");
        assert_eq!(trino_package_dir("Orbit: Cat?"), "Orbit_ Cat_");
    }
}
