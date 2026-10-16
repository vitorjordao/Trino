//! `run n64 --device sc64|everdrive` and `run 3ds --device network`: the
//! real binary against an external project (a `trino.toml` in a temp dir)
//! whose ROM and `.3dsx` already exist (`--no-build`), with stub loaders in
//! place of `sc64deployer`, `UNFLoader` and `3dslink`. Each stub records its
//! command line and prints log lines the way the real tool forwards the
//! console's debug channel; the tests assert the exact command lines and
//! that the lines reach xtask's stdout.
//!
//! No flashcart or 3DS is involved: what the devices themselves do is listed
//! as unverified in `docs/HARDWARE.md`. The stubs are shell scripts, so the
//! deploy tests run on Unix only; the flag and missing-tool checks run
//! everywhere.
#![cfg_attr(not(unix), allow(dead_code))]

use std::path::{Path, PathBuf};
use std::process::{Command, Output};

fn stdout(o: &Output) -> String {
    String::from_utf8_lossy(&o.stdout).into_owned()
}

fn stderr(o: &Output) -> String {
    String::from_utf8_lossy(&o.stderr).into_owned()
}

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

/// An external project with a built ROM, `.3dsx` and 3DS ELF in `target/`.
struct Fixture {
    _tmp: tempfile::TempDir,
    project: PathBuf,
    stubs: PathBuf,
    log: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let project = tmp.path().join("game");
        let stubs = tmp.path().join("stubs");
        for dir in [
            project.join("target/n64"),
            project.join("target/3ds"),
            project.join("target/armv6k-nintendo-3ds/release"),
            stubs.join("devkitpro/libctru"),
        ] {
            std::fs::create_dir_all(dir).unwrap();
        }
        std::fs::write(
            project.join("trino.toml"),
            "[project]\nname = \"demo\"\ntitle = \"Demo\"\n\
             [n64]\npackage = \"demo-n64\"\n\
             [3ds]\npackage = \"demo-3ds\"\nbin = \"demo-3ds\"\n",
        )
        .unwrap();
        std::fs::write(project.join("target/n64/demo.z64"), b"rom").unwrap();
        std::fs::write(project.join("target/3ds/demo.3dsx"), b"app").unwrap();
        std::fs::write(
            project.join("target/armv6k-nintendo-3ds/release/demo-3ds.elf"),
            b"elf",
        )
        .unwrap();
        let log = tmp.path().join("argv.log");
        // Canonical: xtask prints and passes the project's canonical paths.
        let project = project.canonicalize().unwrap();
        Fixture {
            _tmp: tmp,
            project,
            stubs,
            log,
        }
    }

    fn rom(&self) -> String {
        self.project
            .join("target/n64/demo.z64")
            .display()
            .to_string()
    }

    fn app(&self) -> String {
        self.project
            .join("target/3ds/demo.3dsx")
            .display()
            .to_string()
    }

    /// One recorded command line per stub run.
    fn calls(&self) -> Vec<String> {
        std::fs::read_to_string(&self.log)
            .unwrap_or_default()
            .lines()
            .map(String::from)
            .collect()
    }

    /// Run xtask with `--project` (appended unless `args` has it), no tool on PATH but `stub_dir` (if any),
    /// none of the tool overrides but `envs`.
    fn xtask(&self, args: &[&str], envs: &[(&str, &Path)], path: Option<&Path>) -> Output {
        let mut cmd = Command::new(env!("CARGO_BIN_EXE_xtask"));
        cmd.args(args);
        if !args.contains(&"--project") {
            cmd.args(["--project", self.project.to_str().unwrap()]);
        }
        cmd.current_dir(&self.project)
            .env_remove("TRINO_PROJECT")
            .env_remove("TRINO_SC64DEPLOYER")
            .env_remove("TRINO_UNFLOADER")
            .env_remove("TRINO_3DSLINK")
            // A devkitPro install (it has libctru/) without tools: no
            // fallback 3dslink, and no real one from the default places.
            .env("DEVKITPRO", self.stubs.join("devkitpro"))
            .env("STUB_LOG", &self.log);
        // An empty PATH: only the stubs (if given) can be found.
        cmd.env("PATH", path.unwrap_or(Path::new("")));
        for (k, v) in envs {
            cmd.env(k, v);
        }
        cmd.output().expect("spawn xtask")
    }
}

/// A `#!/bin/sh` stub: records `$*`, prints `body`'s output, exits with
/// `$STUB_EXIT` (default 0).
#[cfg(unix)]
fn stub(dir: &Path, name: &str, body: &str) -> PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(
        &path,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$*\" >> \"$STUB_LOG\"\n{body}\nexit ${{STUB_EXIT:-0}}\n"
        ),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path
}

/// SummerCart64: `upload <rom>`, then `debug`, whose output (the ROM's USB
/// log) reaches the terminal. The tool is found on PATH; `--target n64` is
/// the same command as the positional `run n64`, with `--project` anywhere.
#[cfg(unix)]
#[test]
fn sc64_uploads_the_rom_then_forwards_the_debug_log() {
    let f = Fixture::new();
    stub(
        &f.stubs,
        "sc64deployer",
        "case \"$1\" in\n\
         upload) echo 'Uploading ROM... OK' ;;\n\
         debug) echo 'TRINO_FRAME:hub avg_us=16000 max_us=21000 frames=60'; echo 'TRINO_TEST_PASS' ;;\n\
         esac",
    );
    let dir = f.project.to_str().unwrap();
    for args in [
        &["run", "n64", "--device", "sc64", "--no-build"][..],
        &["run", "--target", "n64", "--device", "sc64", "--no-build"],
        // `--project` first, as in `cargo trino --project <dir> run ...`.
        &[
            "--project",
            dir,
            "run",
            "--target",
            "n64",
            "--device",
            "sc64",
            "--no-build",
        ],
    ] {
        let _ = std::fs::remove_file(&f.log);
        let o = f.xtask(args, &[], Some(&f.stubs));
        assert!(o.status.success(), "{args:?}: {}", stderr(&o));
        assert_eq!(f.calls(), [format!("upload {}", f.rom()), "debug".into()]);
        let out = stdout(&o);
        for line in [
            "Uploading ROM... OK",
            "power on (or reset) the N64 now",
            "TRINO_FRAME:hub avg_us=16000 max_us=21000 frames=60",
            "TRINO_TEST_PASS",
        ] {
            assert!(out.contains(line), "stdout lacks `{line}`:\n{out}");
        }
        // The note comes between the upload and the log.
        assert!(out.find("Uploading").unwrap() < out.find("power on").unwrap());
        assert!(out.find("power on").unwrap() < out.find("TRINO_FRAME").unwrap());
    }
}

/// A failed upload stops before `debug` and fails the command.
#[cfg(unix)]
#[test]
fn a_failed_upload_stops_the_deploy() {
    let f = Fixture::new();
    stub(
        &f.stubs,
        "sc64deployer",
        "echo 'Error: No SC64 device found' >&2",
    );
    let fail = PathBuf::from("1");
    let o = f.xtask(
        &["run", "n64", "--device", "sc64", "--no-build"],
        &[("STUB_EXIT", &fail)],
        Some(&f.stubs),
    );
    assert_fails_with(&o, "No SC64 device found");
    assert!(stderr(&o).contains("docs/HARDWARE.md"), "{}", stderr(&o));
    assert_eq!(f.calls(), [format!("upload {}", f.rom())]);
}

/// EverDrive: one UNFLoader call uploads, boots and stays in debug mode
/// with plain stdout (`-b`). The tool comes from `$TRINO_UNFLOADER`.
#[cfg(unix)]
#[test]
fn everdrive_uploads_and_debugs_in_one_unfloader_call() {
    let f = Fixture::new();
    let tool = stub(
        &f.stubs,
        "my-unfloader",
        "echo 'EverDrive X7 found'; echo 'TRINO_FRAME:level avg_us=30000 max_us=33000 frames=60'",
    );
    let o = f.xtask(
        &["run", "n64", "--device", "everdrive", "--no-build"],
        &[("TRINO_UNFLOADER", &tool)],
        None,
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(f.calls(), [format!("-b -f 3 -r {} -d", f.rom())]);
    assert!(stdout(&o).contains("TRINO_FRAME:level avg_us=30000"));
}

/// 3DS: `3dslink -a <ip> -s <app>` keeps a server that prints the app's
/// stdout; `--gdb` writes a Luma3DS gdb script and prints how to use it.
#[cfg(unix)]
#[test]
fn network_deploy_runs_3dslink_with_the_ip_and_writes_the_gdb_script() {
    let f = Fixture::new();
    let tool = stub(
        &f.stubs,
        "3dslink",
        "echo 'Sending demo.3dsx, 3 bytes'; echo 'server active ...'; \
         echo 'TRINO_FRAME:hub avg_us=12000 max_us=15000 frames=60'",
    );
    let o = f.xtask(
        &[
            "run",
            "3ds",
            "--device",
            "network",
            "--ip",
            "192.168.1.50",
            "--no-build",
        ],
        &[("TRINO_3DSLINK", &tool)],
        None,
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(f.calls(), [format!("-a 192.168.1.50 -s {}", f.app())]);
    assert!(stdout(&o).contains("TRINO_FRAME:hub avg_us=12000"));

    // Without --ip, 3dslink finds the 3DS itself; found on PATH this time.
    let _ = std::fs::remove_file(&f.log);
    let o = f.xtask(
        &["run", "3ds", "--device", "network", "--no-build"],
        &[],
        Some(&f.stubs),
    );
    assert!(o.status.success(), "{}", stderr(&o));
    assert_eq!(f.calls(), [format!("-s {}", f.app())]);

    // --gdb: the script loads the ELF and attaches (extended-remote, 4000).
    let o = f.xtask(
        &[
            "run",
            "--target",
            "3ds",
            "--device",
            "network",
            "--ip",
            "192.168.1.50",
            "--gdb",
            "--no-build",
        ],
        &[("TRINO_3DSLINK", &tool)],
        None,
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let script = f.project.join("target/3ds/demo.gdb");
    let text = std::fs::read_to_string(&script).unwrap();
    let elf = f
        .project
        .join("target/armv6k-nintendo-3ds/release/demo-3ds.elf");
    assert!(
        text.contains(&format!("file {}\n", elf.display())),
        "{text}"
    );
    assert!(
        text.contains("target extended-remote 192.168.1.50:4000\n"),
        "{text}"
    );
    assert!(
        stdout(&o).contains(&format!("-x {}", script.display())),
        "{}",
        stdout(&o)
    );

    // --gdb-port 4003: Luma's break-on-start port, plain `target remote`.
    let o = f.xtask(
        &[
            "run",
            "3ds",
            "--device",
            "network",
            "--ip",
            "192.168.1.50",
            "--gdb",
            "--gdb-port",
            "4003",
            "--no-build",
        ],
        &[("TRINO_3DSLINK", &tool)],
        None,
    );
    assert!(o.status.success(), "{}", stderr(&o));
    let text = std::fs::read_to_string(&script).unwrap();
    assert!(text.contains("target remote 192.168.1.50:4003\n"), "{text}");
}

/// Device flags are checked before anything is built or launched, and a
/// missing loader or ROM says what to install or build.
#[test]
fn device_flags_and_missing_tools_fail_with_a_reason() {
    let f = Fixture::new();
    let run = |args: &[&str]| f.xtask(args, &[], None);
    assert_fails_with(
        &run(&["run", "n64", "--device", "cartridge"]),
        "unknown device `cartridge`",
    );
    assert_fails_with(
        &run(&["run", "3ds", "--device", "sc64"]),
        "--device sc64 is a n64 device",
    );
    assert_fails_with(
        &run(&["run", "n64", "--device", "network"]),
        "--device network is a 3ds device",
    );
    assert_fails_with(
        &run(&["test", "n64", "--device", "sc64"]),
        "--device only applies to `run n64` and `run 3ds`",
    );
    assert_fails_with(
        &run(&["run", "pc", "--device", "sc64"]),
        "--device only applies",
    );
    assert_fails_with(&run(&["run", "n64", "--device"]), "--device needs a value");
    assert_fails_with(
        &run(&["run", "3ds", "--ip", "10.0.0.2"]),
        "need --device network",
    );
    assert_fails_with(
        &run(&["run", "3ds", "--device", "network", "--gdb"]),
        "--gdb needs --ip",
    );
    assert_fails_with(
        &run(&[
            "run",
            "3ds",
            "--device",
            "network",
            "--ip",
            "10.0.0.2",
            "--gdb-port",
            "4003",
        ]),
        "--gdb-port needs --gdb",
    );
    assert_fails_with(
        &run(&["run", "n64", "--device", "sc64", "--selftest", "--no-build"]),
        "use one",
    );

    // Missing loaders: install hints.
    assert_fails_with(
        &run(&["run", "n64", "--device", "sc64", "--no-build"]),
        "github.com/Polprzewodnikowy/SummerCart64/releases",
    );
    assert_fails_with(
        &run(&["run", "n64", "--device", "everdrive", "--no-build"]),
        "github.com/buu342/N64-UNFLoader/releases",
    );
    assert_fails_with(
        &run(&["run", "3ds", "--device", "network", "--no-build"]),
        "3dstools",
    );
    let nowhere = f.stubs.join("nowhere");
    assert_fails_with(
        &f.xtask(
            &["run", "3ds", "--device", "network", "--no-build"],
            &[("TRINO_3DSLINK", &nowhere)],
            None,
        ),
        "TRINO_3DSLINK=",
    );
}

/// The `n3ds` alias works in both spellings and deploys like `3ds`.
#[cfg(unix)]
#[test]
fn the_n3ds_alias_deploys_like_3ds() {
    let f = Fixture::new();
    let tool = stub(&f.stubs, "3dslink", "echo 'server active ...'");
    for args in [
        &["run", "n3ds", "--device", "network", "--no-build"][..],
        &[
            "run",
            "--target",
            "n3ds",
            "--device",
            "network",
            "--no-build",
        ],
    ] {
        let _ = std::fs::remove_file(&f.log);
        let o = f.xtask(args, &[("TRINO_3DSLINK", &tool)], None);
        assert!(o.status.success(), "{args:?}: {}", stderr(&o));
        assert_eq!(f.calls(), [format!("-s {}", f.app())]);
    }
}

/// `--no-build` without a ROM points at the build command.
#[cfg(unix)]
#[test]
fn no_build_needs_an_existing_rom() {
    let f = Fixture::new();
    std::fs::remove_file(f.project.join("target/n64/demo.z64")).unwrap();
    let tool = stub(&f.stubs, "sc64deployer", "");
    assert_fails_with(
        &f.xtask(
            &["run", "n64", "--device", "sc64", "--no-build"],
            &[("TRINO_SC64DEPLOYER", &tool)],
            None,
        ),
        "cargo xtask build n64",
    );
    assert!(f.calls().is_empty(), "nothing deployed: {:?}", f.calls());
}
