//! Real hardware: `run n64 --device sc64|everdrive` and `run 3ds --device
//! network [--ip <IP>] [--gdb [--gdb-port N]]` (`docs/HARDWARE.md`).
//!
//! xtask builds as usual (or, with `--no-build`, takes the ROM / `.3dsx`
//! already in `target/`), then hands the file to the device's own loader with
//! the terminal attached: the loader's output, which carries the game's debug
//! channel (every `TRINO_*` line the self-test and `N64Host`/`N3dsHost`
//! print), goes straight to this terminal. The child inherits stdin, stdout
//! and stderr on purpose: piped, both UNFLoader (`vprintf`) and 3dslink
//! (`fwrite`) would block-buffer the log, and `sc64deployer debug` reads
//! stdin until EOF.
//!
//! How the log gets out of the console:
//! - **N64**: the shim calls libdragon's `debug_init_usblog()` next to
//!   `debug_init_isviewer()`, so `debugf` reaches the flashcart's USB port
//!   (UNFLoader protocol), which `sc64deployer debug` and `UNFLoader -d` print.
//! - **3DS**: launched by 3dslink, libctru sets `__3dslink_host`; the shim then
//!   connects stdout/stderr to the host (`link3dsStdio`) and `trino_log`
//!   writes there as well as to `svcOutputDebugString`.
//!
//! Tool lookup: `$TRINO_SC64DEPLOYER` / `$TRINO_UNFLOADER` / `$TRINO_3DSLINK`
//! (like `$TRINO_ARES`), else PATH, else (3dslink) devkitPro's `tools/bin`.

use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};

use crate::n3ds::{exe, which};
use crate::project::Project;

/// Where a `run` goes instead of the emulator.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Device {
    /// SummerCart64 (`sc64deployer`).
    Sc64,
    /// EverDrive 64 X7 / 3.0 (`UNFLoader`).
    EverDrive,
    /// A 3DS on the network running the Homebrew Launcher (`3dslink`).
    Network,
}

impl Device {
    const NAMES: &str = "sc64, everdrive (N64) or network (3DS)";

    fn parse(s: &str) -> Option<Self> {
        match s {
            "sc64" => Some(Device::Sc64),
            "everdrive" => Some(Device::EverDrive),
            "network" => Some(Device::Network),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Device::Sc64 => "sc64",
            Device::EverDrive => "everdrive",
            Device::Network => "network",
        }
    }

    /// The console this device runs (`n64` / `3ds`).
    fn platform(self) -> &'static str {
        match self {
            Device::Sc64 | Device::EverDrive => "n64",
            Device::Network => "3ds",
        }
    }
}

/// Luma3DS's break-on-start port (`target remote`, no "extended-remote").
pub const LUMA_BREAK_ON_START_PORT: u16 = 4003;

/// The `--device` family of flags.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DeviceArgs {
    pub device: Device,
    /// `--ip`: the 3DS's address (3dslink searches the LAN without it).
    pub ip: Option<String>,
    /// `--gdb`: write a gdb script for Luma3DS's GDB stub.
    pub gdb: bool,
    /// `--gdb-port`: 4000-4002 attach to a process, 4003 breaks on start.
    pub gdb_port: u16,
}

/// The issue's `run --target n64` spelling: remove `--target <platform>` and
/// put the platform back where the positional form has it (`run n64`).
/// Only console/PC platform names are taken, so cargo's own `--target
/// <triple>` after `run pc` still passes through.
pub fn normalize_target(args: &mut Vec<String>) -> Result<(), String> {
    let Some(i) = args.iter().position(|a| a == "--target") else {
        return Ok(());
    };
    let Some(value) = args.get(i + 1) else {
        return Err("xtask: --target needs a platform (pc, n64 or 3ds)".into());
    };
    if !matches!(value.as_str(), "pc" | "n64" | "3ds" | "n3ds") {
        return Ok(()); // not ours (e.g. `run pc -- --target <triple>`)
    }
    let platform = args.remove(i + 1);
    args.remove(i);
    if args.is_empty() {
        return Err("xtask: --target needs a command (e.g. run --target n64)".into());
    }
    args.insert(1, platform);
    Ok(())
}

/// Remove a `--flag <value>` pair from `args`.
fn take_value(args: &mut Vec<String>, flag: &str) -> Result<Option<String>, String> {
    let Some(i) = args.iter().position(|a| a == flag) else {
        return Ok(None);
    };
    if i + 1 >= args.len() || args[i + 1].starts_with("--") {
        return Err(format!("xtask: {flag} needs a value"));
    }
    let value = args.remove(i + 1);
    args.remove(i);
    Ok(Some(value))
}

/// Remove `--device`, `--ip`, `--gdb` and `--gdb-port` from `args`.
/// `Ok(None)`: no device flag at all (the emulator path).
pub fn take_device_args(args: &mut Vec<String>) -> Result<Option<DeviceArgs>, String> {
    let device = take_value(args, "--device")?;
    let ip = take_value(args, "--ip")?;
    let gdb_port = take_value(args, "--gdb-port")?;
    let gdb = match args.iter().position(|a| a == "--gdb") {
        Some(i) => {
            args.remove(i);
            true
        }
        None => false,
    };
    let Some(device) = device else {
        if ip.is_some() || gdb || gdb_port.is_some() {
            return Err(
                "xtask: --ip, --gdb and --gdb-port need --device network (see docs/HARDWARE.md)"
                    .into(),
            );
        }
        return Ok(None);
    };
    let device = Device::parse(&device)
        .ok_or_else(|| format!("xtask: unknown device `{device}` — use {}", Device::NAMES))?;
    if device != Device::Network && (ip.is_some() || gdb || gdb_port.is_some()) {
        return Err(format!(
            "xtask: --ip, --gdb and --gdb-port are for --device network, not {}",
            device.name()
        ));
    }
    let gdb_port = match gdb_port {
        Some(p) => p
            .parse::<u16>()
            .map_err(|_| format!("xtask: --gdb-port `{p}` is not a port number"))?,
        None => LUMA_BREAK_ON_START_PORT,
    };
    if gdb && ip.is_none() {
        return Err("xtask: --gdb needs --ip (the address gdb connects to)".into());
    }
    Ok(Some(DeviceArgs {
        device,
        ip,
        gdb,
        gdb_port,
    }))
}

/// A device flag is only meaningful on `run n64` / `run 3ds`, for the
/// device's own console.
pub fn check_invocation(
    cmd: Option<&str>,
    platform: Option<&str>,
    d: &DeviceArgs,
) -> Result<(), String> {
    let platform = platform.map(|p| if p == "n3ds" { "3ds" } else { p });
    match (cmd, platform) {
        (Some("run"), Some(p @ ("n64" | "3ds"))) if p == d.device.platform() => Ok(()),
        (Some("run"), Some(p @ ("n64" | "3ds"))) => Err(format!(
            "xtask: --device {} is a {} device, not {p} (use {})",
            d.device.name(),
            d.device.platform(),
            Device::NAMES
        )),
        _ => Err(format!(
            "xtask: --device only applies to `run n64` and `run 3ds` (got `{}{}`)",
            cmd.unwrap_or(""),
            platform.map(|p| format!(" {p}")).unwrap_or_default()
        )),
    }
}

/// A host tool a device needs.
struct Tool {
    /// Executable name (`.exe` added on Windows).
    name: &'static str,
    /// Override: a path to the executable.
    env: &'static str,
    /// What to do when it is missing.
    install: &'static str,
}

const SC64DEPLOYER: Tool = Tool {
    name: "sc64deployer",
    env: "TRINO_SC64DEPLOYER",
    install: "download sc64-deployer-<os>-<version> from \
              https://github.com/Polprzewodnikowy/SummerCart64/releases and put \
              sc64deployer on PATH (`sc64deployer list` should show the cart)",
};

const UNFLOADER: Tool = Tool {
    name: "UNFLoader",
    env: "TRINO_UNFLOADER",
    install: "download it from https://github.com/buu342/N64-UNFLoader/releases \
              (Linux: run UNFLoader/installer_linux.sh for the FTDI driver and USB \
              permissions, or run it with sudo) and put UNFLoader on PATH",
};

const LINK3DS: Tool = Tool {
    name: "3dslink",
    env: "TRINO_3DSLINK",
    install: "it ships with devkitPro's 3DS tools (`dkp-pacman -S 3dstools`, part \
              of 3ds-dev); set DEVKITPRO or put 3dslink on PATH",
};

/// `$<env>` (must exist), else PATH, else devkitPro's `tools/bin` for
/// 3dslink.
fn find_tool(tool: &Tool) -> Result<PathBuf, String> {
    if let Some(env) = std::env::var_os(tool.env).filter(|v| !v.is_empty()) {
        let p = PathBuf::from(&env);
        return if p.is_file() {
            Ok(p)
        } else {
            Err(format!(
                "{}={} does not exist — point it at the {} executable",
                tool.env,
                p.display(),
                tool.name
            ))
        };
    }
    if let Some(p) = which(&exe(tool.name)) {
        return Ok(p);
    }
    if tool.name == LINK3DS.name {
        // $DEVKITPRO when set (its install wins), else the default places.
        let roots: Vec<PathBuf> = match std::env::var_os("DEVKITPRO").filter(|v| !v.is_empty()) {
            Some(d) => vec![PathBuf::from(d)],
            None => vec![
                PathBuf::from(r"C:\devkitPro"),
                PathBuf::from("/opt/devkitpro"),
            ],
        };
        if let Some(p) = roots
            .iter()
            .map(|r| r.join("tools/bin").join(exe(tool.name)))
            .find(|p| p.is_file())
        {
            return Ok(p);
        }
    }
    Err(format!(
        "{} not found (set {} to the executable) — {}",
        tool.name, tool.env, tool.install
    ))
}

/// One loader invocation.
#[derive(Debug, PartialEq, Eq)]
pub struct Step {
    pub args: Vec<OsString>,
    /// Printed before the step runs (what the user does on the console).
    pub note: Option<&'static str>,
}

fn step(args: &[&dyn AsRef<std::ffi::OsStr>], note: Option<&'static str>) -> Step {
    Step {
        args: args.iter().map(|a| a.as_ref().to_owned()).collect(),
        note,
    }
}

/// The loader command lines for an N64 ROM.
///
/// - SummerCart64 (`sc64deployer`, SummerCart64 `docs/00_quick_startup_guide.md`
///   and `sw/deployer/src/main.rs`): `upload <rom>` writes the ROM (save type
///   from the header) but never boots the console — `--reboot` needs support
///   in the running game, so the user powers on / resets the N64 — then
///   `debug` prints the USB debug channel until Ctrl+C. No `--isv`: the ROM
///   already logs over USB, ISViewer would print every line twice.
/// - EverDrive (`UNFLoader`, `UNFLoader/main.cpp` in buu342/N64-UNFLoader):
///   `-b` plain stdout instead of the ncurses UI, `-f 3` forces the
///   "EverDrive 3.0 or X7" cart type, `-r <rom>` uploads and boots, `-d`
///   stays in debug mode (last: it takes an optional log-file name).
pub fn n64_steps(device: Device, rom: &Path) -> Vec<Step> {
    match device {
        Device::Sc64 => vec![
            step(&[&"upload", &rom], None),
            step(
                &[&"debug"],
                Some("power on (or reset) the N64 now; Ctrl+C stops the log"),
            ),
        ],
        Device::EverDrive => vec![step(&[&"-b", &"-f", &"3", &"-r", &rom, &"-d"], None)],
        Device::Network => Vec::new(),
    }
}

/// `3dslink` (devkitPro/3dslink `host/src/main.c`, `3dslink --help`):
/// `-a <ip>` addresses the 3DS (without it 3dslink broadcasts to find one),
/// `-s` keeps a server on TCP 17491 after the upload and prints what the app
/// writes to stdout/stderr until it exits.
pub fn n3ds_step(ip: Option<&str>, app: &Path) -> Step {
    match ip {
        Some(ip) => step(&[&"-a", &ip, &"-s", &app], None),
        None => step(&[&"-s", &app], None),
    }
}

/// A gdb script for Luma3DS's GDB stub (Luma3DS README: ports 4000-4002 are
/// the normal ports, used with `attach` in extended-remote mode; 4003 is the
/// break-on-start port, used without extended-remote).
pub fn gdb_script(elf: &Path, ip: &str, port: u16) -> String {
    let elf = elf.display().to_string().replace('\\', "/");
    let mut s = format!(
        "# Generated by `cargo xtask run 3ds --device network --gdb` (docs/HARDWARE.md).\n\
         # Luma3DS: open Rosalina (L+Down+Select) > Debugger options > Enable debugger.\n\
         file {elf}\n"
    );
    if port == LUMA_BREAK_ON_START_PORT {
        s.push_str(
            "# Break on start: pick \"Force-debug next application at launch\" before the\n\
             # upload; the app stops before its first instruction.\n",
        );
        s.push_str(&format!("target remote {ip}:{port}\n"));
    } else {
        s.push_str(
            "# Attach: the app is running; find it and attach:\n\
             #   info os processes\n\
             #   attach <pid>\n",
        );
        s.push_str(&format!("target extended-remote {ip}:{port}\n"));
    }
    s
}

/// devkitARM's gdb when devkitPro is installed, else the one on PATH.
fn arm_gdb() -> PathBuf {
    let name = exe("arm-none-eabi-gdb");
    std::env::var_os("DEVKITPRO")
        .map(|d| PathBuf::from(d).join("devkitARM/bin").join(&name))
        .filter(|p| p.is_file())
        .or_else(|| which(&name))
        .unwrap_or_else(|| PathBuf::from(name))
}

fn shell_line(tool: &Path, args: &[OsString]) -> String {
    let mut line = tool.display().to_string();
    for a in args {
        line.push(' ');
        line.push_str(&a.to_string_lossy());
    }
    line
}

/// Run each step with the terminal attached; stop at the first failure.
fn run_steps(tag: &str, tool: &Path, steps: &[Step]) -> Result<(), String> {
    for s in steps {
        if let Some(note) = s.note {
            println!("[{tag}] {note}");
        }
        println!("[{tag}] $ {}", shell_line(tool, &s.args));
        let status = Command::new(tool)
            .args(&s.args)
            .status()
            .map_err(|e| format!("{}: {e}", tool.display()))?;
        if !status.success() {
            return Err(format!(
                "`{}` failed ({status}) — see docs/HARDWARE.md › Troubleshooting",
                shell_line(tool, &s.args)
            ));
        }
    }
    Ok(())
}

/// The ROM / app to deploy: built now, or the one in `target/` (`--no-build`).
fn artifact(path: PathBuf, build: impl FnOnce() -> ExitCode, hint: &str) -> Result<PathBuf, ()> {
    if build() != ExitCode::SUCCESS {
        return Err(());
    }
    if !path.is_file() {
        eprintln!("{} missing ({hint})", path.display());
        return Err(());
    }
    Ok(path)
}

fn finish(tag: &str, r: Result<(), String>) -> ExitCode {
    match r {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[{tag}] {e}");
            ExitCode::FAILURE
        }
    }
}

/// `run n64 --device sc64|everdrive`.
pub fn deploy_n64(p: &Project, d: &DeviceArgs, build_first: bool, selftest: bool) -> ExitCode {
    let tool = match d.device {
        Device::Sc64 => &SC64DEPLOYER,
        _ => &UNFLOADER,
    };
    // Find the tool first: no point building for a loader that isn't there.
    let exe = match find_tool(tool) {
        Ok(t) => t,
        Err(e) => return finish("n64", Err(e)),
    };
    let build = || {
        if build_first {
            crate::n64::build(p, selftest)
        } else {
            ExitCode::SUCCESS
        }
    };
    let Ok(rom) = artifact(p.rom(), build, "cargo xtask build n64") else {
        return ExitCode::FAILURE;
    };
    println!(
        "[n64] deploying {} to the {}",
        rom.display(),
        d.device.name()
    );
    finish("n64", run_steps("n64", &exe, &n64_steps(d.device, &rom)))
}

/// `run 3ds --device network [--ip <IP>] [--gdb [--gdb-port N]]`.
pub fn deploy_3ds(p: &Project, d: &DeviceArgs, build_first: bool, selftest: bool) -> ExitCode {
    let exe = match find_tool(&LINK3DS) {
        Ok(t) => t,
        Err(e) => return finish("3ds", Err(e)),
    };
    let build = || {
        if build_first {
            crate::n3ds::build(p, selftest)
        } else {
            ExitCode::SUCCESS
        }
    };
    let Ok(app) = artifact(p.app_3dsx(), build, "cargo xtask build 3ds") else {
        return ExitCode::FAILURE;
    };
    if d.gdb {
        let ip =
            d.ip.as_deref()
                .expect("--gdb requires --ip (checked when parsed)");
        let elf = p.n3ds_elf();
        if !elf.is_file() {
            eprintln!(
                "[3ds] warning: {} missing — gdb will have no symbols",
                elf.display()
            );
        }
        let script = p.root.join(format!("target/3ds/{}.gdb", p.name));
        if let Err(e) = std::fs::write(&script, gdb_script(&elf, ip, d.gdb_port)) {
            return finish("3ds", Err(format!("{}: {e}", script.display())));
        }
        println!("[3ds] gdb script: {}", script.display());
        println!(
            "[3ds] in another terminal: {} -x {}",
            arm_gdb().display(),
            script.display()
        );
    }
    println!(
        "[3ds] open the Homebrew Launcher and press Y (netloader) on the 3DS; \
         Ctrl+C stops the log"
    );
    finish(
        "3ds",
        run_steps("3ds", &exe, &[n3ds_step(d.ip.as_deref(), &app)]),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(s: &str) -> Vec<String> {
        s.split_whitespace().map(String::from).collect()
    }

    #[test]
    fn target_flag_becomes_the_positional_platform() {
        let mut a = args("run --target n64 --device sc64");
        normalize_target(&mut a).unwrap();
        assert_eq!(a, args("run n64 --device sc64"));
        let mut a = args("run --device network --target 3ds");
        normalize_target(&mut a).unwrap();
        assert_eq!(a, args("run 3ds --device network"));
        // cargo's own --target after `run pc` is left alone.
        let mut a = args("run pc --target x86_64-unknown-linux-gnu");
        normalize_target(&mut a).unwrap();
        assert_eq!(a, args("run pc --target x86_64-unknown-linux-gnu"));
        assert!(normalize_target(&mut args("run --target")).is_err());
    }

    #[test]
    fn device_flags_parse_and_reject_bad_combinations() {
        let mut a = args("run n64 --device sc64");
        let d = take_device_args(&mut a).unwrap().unwrap();
        assert_eq!((d.device, a), (Device::Sc64, args("run n64")));

        let mut a = args("run 3ds --device network --ip 10.0.0.2 --gdb --gdb-port 4000");
        let d = take_device_args(&mut a).unwrap().unwrap();
        assert_eq!(a, args("run 3ds"));
        assert_eq!(
            d,
            DeviceArgs {
                device: Device::Network,
                ip: Some("10.0.0.2".into()),
                gdb: true,
                gdb_port: 4000,
            }
        );
        let d = take_device_args(&mut args("run 3ds --device network --ip 1.2.3.4 --gdb"))
            .unwrap()
            .unwrap();
        assert_eq!(d.gdb_port, LUMA_BREAK_ON_START_PORT);

        assert_eq!(take_device_args(&mut args("run n64")).unwrap(), None);
        for bad in [
            "run n64 --device",
            "run n64 --device cartridge",
            "run n64 --device sc64 --ip 1.2.3.4",
            "run 3ds --ip 1.2.3.4",
            "run 3ds --device network --gdb",
            "run 3ds --device network --ip 1.2.3.4 --gdb-port x",
        ] {
            assert!(take_device_args(&mut args(bad)).is_err(), "{bad}");
        }
    }

    #[test]
    fn devices_only_run_on_their_own_console() {
        let d = |device| DeviceArgs {
            device,
            ip: None,
            gdb: false,
            gdb_port: LUMA_BREAK_ON_START_PORT,
        };
        assert!(check_invocation(Some("run"), Some("n64"), &d(Device::Sc64)).is_ok());
        assert!(check_invocation(Some("run"), Some("n64"), &d(Device::EverDrive)).is_ok());
        assert!(check_invocation(Some("run"), Some("3ds"), &d(Device::Network)).is_ok());
        assert!(check_invocation(Some("run"), Some("n3ds"), &d(Device::Network)).is_ok());
        assert!(check_invocation(Some("run"), Some("3ds"), &d(Device::Sc64)).is_err());
        assert!(check_invocation(Some("run"), Some("n64"), &d(Device::Network)).is_err());
        assert!(check_invocation(Some("test"), Some("n64"), &d(Device::Sc64)).is_err());
        assert!(check_invocation(Some("run"), Some("pc"), &d(Device::Sc64)).is_err());
        assert!(check_invocation(Some("run"), None, &d(Device::Sc64)).is_err());
    }

    #[test]
    fn loader_command_lines() {
        let rom = Path::new("/p/target/n64/game.z64");
        let strs = |s: &Step| -> Vec<String> {
            s.args
                .iter()
                .map(|a| a.to_string_lossy().into_owned())
                .collect()
        };
        let sc64 = n64_steps(Device::Sc64, rom);
        assert_eq!(sc64.len(), 2);
        assert_eq!(strs(&sc64[0]), ["upload", "/p/target/n64/game.z64"]);
        assert_eq!(strs(&sc64[1]), ["debug"]);
        let ed = n64_steps(Device::EverDrive, rom);
        assert_eq!(
            strs(&ed[0]),
            ["-b", "-f", "3", "-r", "/p/target/n64/game.z64", "-d"]
        );
        let app = Path::new("/p/target/3ds/game.3dsx");
        assert_eq!(
            strs(&n3ds_step(Some("10.0.0.2"), app)),
            ["-a", "10.0.0.2", "-s", "/p/target/3ds/game.3dsx"]
        );
        assert_eq!(
            strs(&n3ds_step(None, app)),
            ["-s", "/p/target/3ds/game.3dsx"]
        );
    }

    #[test]
    fn gdb_scripts_follow_lumas_ports() {
        let elf = Path::new("/p/game.elf");
        let start = gdb_script(elf, "10.0.0.2", 4003);
        assert!(start.contains("file /p/game.elf\n"));
        assert!(start.contains("target remote 10.0.0.2:4003\n"));
        let attach = gdb_script(elf, "10.0.0.2", 4000);
        assert!(attach.contains("target extended-remote 10.0.0.2:4000\n"));
        assert!(attach.contains("attach <pid>"));
    }
}
