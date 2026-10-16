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

/// Emulator idle limit: `$TRINO_EMU_IDLE` seconds without any output line
/// (default 120, `0` = off). A self-test prints a line per step, so a long
/// silence is a hang, while a slow machine still makes progress.
pub fn idle_secs() -> u64 {
    std::env::var("TRINO_EMU_IDLE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(120)
}

/// The two limits of an emulator self-test run: the whole run
/// ([`timeout_secs`]) and the silence between two output lines
/// ([`idle_secs`]). A failure names which one ran out and the last step the
/// test reached, so a hang (silence) and a slow run (still printing at the
/// deadline) read differently in a log.
pub struct Watchdog {
    start: std::time::Instant,
    total: std::time::Duration,
    idle: Option<std::time::Duration>,
    last_line: std::time::Instant,
    last_step: String,
}

impl Watchdog {
    pub fn new() -> Self {
        Self::with_limits(timeout_secs(), idle_secs(), std::time::Instant::now())
    }

    pub fn with_limits(total_secs: u64, idle_secs: u64, now: std::time::Instant) -> Self {
        Watchdog {
            start: now,
            total: std::time::Duration::from_secs(total_secs),
            idle: (idle_secs > 0).then(|| std::time::Duration::from_secs(idle_secs)),
            last_line: now,
            last_step: String::new(),
        }
    }

    /// A line arrived at `now`; `TRINO_STEP:` lines become the last step.
    pub fn saw(&mut self, line: &str, now: std::time::Instant) {
        self.last_line = now;
        if let Some(i) = line.find("TRINO_STEP:") {
            self.last_step = line[i + "TRINO_STEP:".len()..].trim().to_string();
        }
    }

    /// Time left before a limit runs out (for receive timeouts).
    pub fn left(&self, now: std::time::Instant) -> std::time::Duration {
        let total = (self.start + self.total).saturating_duration_since(now);
        match self.idle {
            Some(idle) => total.min((self.last_line + idle).saturating_duration_since(now)),
            None => total,
        }
    }

    /// `Some(reason)` once a limit ran out at `now`.
    pub fn expired(&self, now: std::time::Instant) -> Option<String> {
        let step = if self.last_step.is_empty() {
            "before the first step".to_string()
        } else {
            format!("after step `{}`", self.last_step)
        };
        let ran = now.saturating_duration_since(self.start).as_secs();
        let quiet = now.saturating_duration_since(self.last_line).as_secs();
        if let Some(idle) = self.idle
            && now.saturating_duration_since(self.last_line) >= idle
        {
            return Some(format!(
                "stalled: no output for {quiet}s {step} ({ran}s into the run): \
                 a hang, not a slow run (TRINO_EMU_IDLE={})",
                idle.as_secs()
            ));
        }
        if now.saturating_duration_since(self.start) >= self.total {
            return Some(format!(
                "timeout: no TRINO_TEST_* output within {}s; still printing \
                 (last line {quiet}s ago, {step}): a slow run, raise TRINO_EMU_TIMEOUT",
                self.total.as_secs()
            ));
        }
        None
    }
}

impl Default for Watchdog {
    fn default() -> Self {
        Self::new()
    }
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

#[cfg(test)]
mod tests {
    use super::Watchdog;
    use std::time::{Duration, Instant};

    #[test]
    fn a_silent_emulator_is_reported_as_a_hang() {
        let t0 = Instant::now();
        let mut w = Watchdog::with_limits(300, 120, t0);
        w.saw("TRINO_STEP:event", t0 + Duration::from_secs(10));
        assert_eq!(w.expired(t0 + Duration::from_secs(129)), None);
        let why = w.expired(t0 + Duration::from_secs(130)).unwrap();
        assert!(why.starts_with("stalled"), "{why}");
        assert!(why.contains("after step `event`"), "{why}");
    }

    #[test]
    fn a_slow_but_printing_run_is_reported_as_a_timeout() {
        let t0 = Instant::now();
        let mut w = Watchdog::with_limits(300, 120, t0);
        for s in (0..300).step_by(60) {
            w.saw("[ares] TRINO_STEP:stress", t0 + Duration::from_secs(s));
            assert_eq!(w.expired(t0 + Duration::from_secs(s + 30)), None);
        }
        w.saw("TRINO_FRAME:stress avg_us=1", t0 + Duration::from_secs(299));
        let why = w.expired(t0 + Duration::from_secs(300)).unwrap();
        assert!(why.starts_with("timeout"), "{why}");
        assert!(why.contains("step `stress`"), "{why}");
        // The receive timeout never overshoots either limit.
        assert_eq!(w.left(t0 + Duration::from_secs(300)), Duration::ZERO);
    }

    #[test]
    fn idle_zero_turns_the_silence_limit_off() {
        let t0 = Instant::now();
        let w = Watchdog::with_limits(300, 0, t0);
        assert_eq!(w.expired(t0 + Duration::from_secs(299)), None);
        assert!(w.expired(t0 + Duration::from_secs(300)).is_some());
    }
}
