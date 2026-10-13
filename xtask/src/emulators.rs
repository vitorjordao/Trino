//! `cargo xtask emulators setup ares|azahar`: the emulators the console
//! self-tests boot, provisioned the same way locally and in CI.
//!
//! - **ares** (N64): built from source into the `trino-ares` Docker image
//!   (`docker/ares/Dockerfile`; Xvfb + software Vulkan). Use it with
//!   `TRINO_ARES=docker cargo xtask test n64`.
//! - **Azahar** (3DS, Linux x86_64): the pinned release AppImage is
//!   downloaded, its SHA-256 verified and extracted (no FUSE needed) into
//!   `target/emulators/azahar/`, where `cargo xtask test 3ds` finds it.
//!   Headless: run the test under `xvfb-run -a`.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

pub const AZAHAR_VERSION: &str = "2126.2";
const AZAHAR_URL: &str =
    "https://github.com/azahar-emu/azahar/releases/download/2126.2/azahar.AppImage";
const AZAHAR_SHA256: &str = "af037f8b775198f636b14bcfe63fb401b02246ebfefe1025bb905ce042556960";

/// Docker image tag `TRINO_ARES=docker` runs.
pub const ARES_IMAGE: &str = "trino-ares";

/// Emulator self-test timeout: `$TRINO_EMU_TIMEOUT` seconds, default 300.
/// CI raises it — software GPU emulation on 4-core runners is slower.
pub fn timeout_secs() -> u64 {
    std::env::var("TRINO_EMU_TIMEOUT")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(300)
}

/// Where `setup azahar` extracts the emulator.
pub fn azahar_dir(root: &Path) -> PathBuf {
    root.join("target/emulators/azahar")
}

/// The extracted Azahar entry point, if set up.
pub fn local_azahar(root: &Path) -> Option<PathBuf> {
    let p = azahar_dir(root).join("squashfs-root/AppRun");
    p.exists().then_some(p)
}

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let result = match args {
        ["setup", "ares"] => setup_ares(root),
        ["setup", "azahar"] => setup_azahar(root),
        _ => Err("usage: cargo xtask emulators setup ares|azahar".to_string()),
    };
    match result {
        Ok(msg) => {
            println!("[emulators] {msg}");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[emulators] {e}");
            ExitCode::FAILURE
        }
    }
}

fn status(cmd: &mut Command) -> Result<(), String> {
    let what = format!("{cmd:?}");
    let s = cmd.status().map_err(|e| format!("{what}: {e}"))?;
    s.success()
        .then_some(())
        .ok_or_else(|| format!("{what} failed ({s})"))
}

fn setup_ares(root: &Path) -> Result<String, String> {
    status(
        Command::new("docker")
            .args(["build", "-t", ARES_IMAGE])
            .arg(root.join("docker/ares")),
    )?;
    Ok(format!(
        "image `{ARES_IMAGE}` ready — run `TRINO_ARES=docker cargo xtask test n64`"
    ))
}

fn setup_azahar(root: &Path) -> Result<String, String> {
    if !cfg!(all(target_os = "linux", target_arch = "x86_64")) {
        return Err(
            "the pinned AppImage is Linux x86_64 only; elsewhere install Azahar and set \
             TRINO_AZAHAR"
                .into(),
        );
    }
    let dir = azahar_dir(root);
    if let Some(p) = local_azahar(root) {
        return Ok(format!(
            "Azahar {AZAHAR_VERSION} already at {}",
            p.display()
        ));
    }
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let image = dir.join("azahar.AppImage");
    status(
        Command::new("curl")
            .args(["-fsSL", "--retry", "3", "-o"])
            .arg(&image)
            .arg(AZAHAR_URL),
    )?;
    let out = Command::new("sha256sum")
        .arg(&image)
        .output()
        .map_err(|e| format!("sha256sum: {e}"))?;
    let digest = String::from_utf8_lossy(&out.stdout);
    let digest = digest.split_whitespace().next().unwrap_or("");
    if digest != AZAHAR_SHA256 {
        let _ = std::fs::remove_file(&image);
        return Err(format!(
            "Azahar AppImage checksum mismatch: got {digest}, expected {AZAHAR_SHA256}"
        ));
    }
    set_executable(&image)?;
    // Extract instead of mounting: containers have no FUSE.
    status(
        Command::new(&image)
            .arg("--appimage-extract")
            .current_dir(&dir)
            .stdout(std::process::Stdio::null()),
    )?;
    let app = local_azahar(root).ok_or("extraction produced no squashfs-root/AppRun")?;
    Ok(format!(
        "Azahar {AZAHAR_VERSION} ready at {}",
        app.display()
    ))
}

#[cfg(unix)]
fn set_executable(p: &Path) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(p, std::fs::Permissions::from_mode(0o755)).map_err(|e| e.to_string())
}

#[cfg(not(unix))]
fn set_executable(_: &Path) -> Result<(), String> {
    Ok(())
}
