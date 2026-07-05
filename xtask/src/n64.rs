//! N64 pipeline: Docker toolchain image -> C shim (o64) -> host cargo
//! (nightly, build-std, staticlib, o32) -> in-container link with
//! mips64-elf-g++ + n64.ld -> ROM packing -> ares.
//!
//! Every native step runs inside the pinned image (`docker/n64/Dockerfile`);
//! only rustc runs on the host. On Windows, docker is reached through WSL2.
//!
//! ABI: libdragon is -mabi=o64, LLVM has no o64, so Rust codegens o32
//! (+noabicalls) and GNU ld links the mix (see .cargo/config.toml note).

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode, Stdio};
use std::time::{Duration, Instant};

use trino_asset_pipeline::{Manifest, Platform, resolve_source};

const IMAGE: &str = "trino-n64";
const TARGET_JSON: &str = "platforms/n64/mips-nintendo64-none.json";
const STATICLIB: &str = "target/mips-nintendo64-none/release/libtrino_app_n64.a";

pub fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("repo root")
}

/// Repo root as the docker host path (WSL mount path on Windows).
fn docker_volume_path(root: &Path) -> String {
    let s = root.to_string_lossy().replace('\\', "/");
    if let Some(rest) = s.strip_prefix("//?/") {
        // canonicalize() yields \\?\C:\... on Windows.
        let (drive, tail) = rest.split_once(':').expect("drive path");
        format!("/mnt/{}{}", drive.to_lowercase(), tail)
    } else if let Some((drive, tail)) = s.split_once(':') {
        format!("/mnt/{}{}", drive.to_lowercase(), tail)
    } else {
        s.to_string() // already a unix path (Linux CI)
    }
}

/// Run a shell script inside the toolchain image with the repo mounted at
/// /workdir. The script goes through a file, never through the command
/// line: on Windows, wsl.exe re-interprets arguments with its own shell,
/// which would expand `$VARS` outside the container.
fn docker_sh(root: &Path, script: &str) -> std::io::Result<std::process::ExitStatus> {
    let script_dir = root.join("target/n64");
    std::fs::create_dir_all(&script_dir)?;
    // \n only: bash inside the container chokes on \r.
    std::fs::write(
        script_dir.join(".docker-step.sh"),
        script.replace("\r\n", "\n"),
    )?;

    let vol = format!("{}:/workdir", docker_volume_path(root));
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("wsl");
        c.arg("docker");
        c
    } else {
        Command::new("docker")
    };
    cmd.args([
        "run",
        "--rm",
        "-v",
        &vol,
        "-w",
        "/workdir",
        IMAGE,
        "bash",
        "/workdir/target/n64/.docker-step.sh",
    ])
    .status()
}

fn image_exists() -> bool {
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("wsl");
        c.arg("docker");
        c
    } else {
        Command::new("docker")
    };
    cmd.args(["image", "inspect", IMAGE])
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ensure_image(root: &Path) -> Result<(), String> {
    if image_exists() {
        return Ok(());
    }
    println!("[n64] building toolchain image (first time, ~10 min)...");
    let ctx = docker_volume_path(&root.join("docker/n64"));
    let mut cmd = if cfg!(windows) {
        let mut c = Command::new("wsl");
        c.arg("docker");
        c
    } else {
        Command::new("docker")
    };
    let status = cmd
        .args(["build", "-t", IMAGE, &ctx])
        .status()
        .map_err(|e| format!("docker not reachable: {e}"))?;
    if !status.success() {
        return Err("docker build failed".into());
    }
    Ok(())
}

/// Compile the C shim inside the image (real libdragon headers + o64 ABI).
fn compile_shim(root: &Path) -> Result<(), String> {
    let shim_src = root.join("crates/platform-n64/shim/trino_shim.c");
    let shim_obj = root.join("target/n64/shim.o");
    if let (Ok(src_meta), Ok(obj_meta)) = (shim_src.metadata(), shim_obj.metadata())
        && let (Ok(src_time), Ok(obj_time)) = (src_meta.modified(), obj_meta.modified())
        && obj_time >= src_time
    {
        return Ok(());
    }
    println!("[n64] compiling C shim...");
    let script = "set -e\n\
        mkdir -p target/n64\n\
        mips64-elf-gcc -c -std=gnu17 -march=vr4300 -mtune=vr4300 -mabi=o64 -O2 -G0 \
        -ffunction-sections -fdata-sections -DN64 \
        -I$N64_INST/mips64-elf/include \
        crates/platform-n64/shim/trino_shim.c -o target/n64/shim.o";
    let status = docker_sh(root, script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("shim compilation failed".into());
    }
    Ok(())
}

/// Bake N64 assets with the image's mksprite/audioconv64 into
/// target/n64/filesystem, plus the index.tsv the runtime reads at boot.
pub fn bake_assets(root: &Path, test_mode: bool) -> Result<(), String> {
    ensure_image(root)?;
    let manifest = Manifest::load(&root.join("assets/manifest.toml"))?;

    let stage = root.join("target/n64/stage");
    let fsdir = root.join("target/n64/filesystem");
    let _ = std::fs::remove_dir_all(&stage);
    let _ = std::fs::remove_dir_all(&fsdir);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&fsdir).map_err(|e| e.to_string())?;

    let mut index = String::new();
    let mut script = String::from("set -e\n");

    for (name, decl) in &manifest.sprites {
        let logical = format!("sprites/{name}");
        let id = trino_core::asset_id(&logical);
        let format = decl.format_for(&logical, Platform::N64)?;
        // Manifest names follow the hardware format; mksprite has its own
        // vocabulary (RGBA5551 is 16-bit color = RGBA16 there).
        let mksprite_format = match format.as_str() {
            "RGBA5551" => "RGBA16",
            other => other, // CI4 / CI8 match 1:1
        };
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        std::fs::copy(&source, stage.join(format!("{id:08x}.png"))).map_err(|e| e.to_string())?;
        script.push_str(&format!(
            "mksprite --format {mksprite_format} -o target/n64/filesystem target/n64/stage/{id:08x}.png\n"
        ));
        index.push_str(&format!("{id:08x}\tsprite\t{id:08x}.sprite\n"));
    }
    for (name, decl) in &manifest.sounds {
        let logical = format!("sounds/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        std::fs::copy(&source, stage.join(format!("{id:08x}.wav"))).map_err(|e| e.to_string())?;
        script.push_str(&format!(
            "audioconv64 -o target/n64/filesystem target/n64/stage/{id:08x}.wav\n"
        ));
        index.push_str(&format!("{id:08x}\tsound\t{id:08x}.wav64\n"));
    }
    for (name, decl) in &manifest.music {
        let logical = format!("music/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        std::fs::copy(&source, stage.join(format!("{id:08x}.wav"))).map_err(|e| e.to_string())?;
        // Loop metadata is baked into the wav64 on the N64; the runtime's
        // `looped` flag is a no-op there (see the shim).
        script.push_str(&format!(
            "audioconv64 --wav-loop true -o target/n64/filesystem target/n64/stage/{id:08x}.wav\n"
        ));
        index.push_str(&format!("{id:08x}\tmusic\t{id:08x}.wav64\n"));
    }
    for (name, decl) in &manifest.models {
        let logical = format!("models/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        // TMDL is portable: same blob on every platform, no container tool.
        let blob = trino_asset_pipeline::bake_model_tmdl(&source)?;
        std::fs::write(fsdir.join(format!("{id:08x}.tmdl")), blob).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tmodel\t{id:08x}.tmdl\n"));
    }

    std::fs::write(fsdir.join("index.tsv"), index).map_err(|e| e.to_string())?;
    if test_mode {
        std::fs::write(fsdir.join("test_mode"), "1").map_err(|e| e.to_string())?;
    }

    println!("[n64] baking assets in container...");
    let status = docker_sh(root, &script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("asset baking failed".into());
    }
    Ok(())
}

fn cargo_build(root: &Path) -> Result<(), String> {
    println!("[n64] building Rust (nightly, build-std)...");
    let status = Command::new("cargo")
        .args([
            "+nightly",
            "build",
            "--release",
            "-Zbuild-std=core,alloc",
            "-Zjson-target-spec",
            "--target",
            TARGET_JSON,
            "-p",
            "trino-app-n64",
        ])
        .current_dir(root)
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err("N64 Rust build failed".into());
    }
    Ok(())
}

/// In-container link (mirroring n64.mk's %.elf recipe, plus
/// --no-warn-mismatch for the o32 staticlib + o64 libdragon mix), then
/// n64sym + strip + n64elfcompress + mkdfs + n64tool, mirroring the %.z64
/// recipe.
fn pack_rom(root: &Path) -> Result<(), String> {
    println!("[n64] linking + packing ROM...");
    let script = format!(
        "set -e\n\
         mips64-elf-g++ -o target/n64/trino.elf target/n64/shim.o {STATICLIB} \
         -lc -mabi=o64 -Wl,-g -Wl,-L$N64_INST/mips64-elf/lib \
         -Wl,-ldragon -Wl,-lm -Wl,-ldragonsys -Wl,-Tn64.ld \
         -Wl,--gc-sections -Wl,--no-warn-mismatch \
         -Wl,--wrap -Wl,__do_global_ctors \
         -Wl,-Map=target/n64/trino.map\n\
         mips64-elf-size -G target/n64/trino.elf\n\
         cd target/n64\n\
         n64sym trino.elf trino.elf.sym\n\
         cp trino.elf trino.elf.stripped\n\
         mips64-elf-strip -s trino.elf.stripped\n\
         n64elfcompress -o . -c 1 trino.elf.stripped\n\
         mkdfs trino.dfs filesystem >/dev/null\n\
         n64tool --title Trino --toc --output trino.z64 --align 256 trino.elf.stripped \
         --align 8 trino.elf.sym --align 16 trino.dfs\n"
    );
    let status = docker_sh(root, &script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("ROM packing failed".into());
    }
    println!("[n64] ROM: target/n64/trino.z64");
    Ok(())
}

pub fn build(test_mode: bool) -> ExitCode {
    let root = repo_root();
    let steps = || -> Result<(), String> {
        ensure_image(&root)?;
        compile_shim(&root)?;
        bake_assets(&root, test_mode)?;
        cargo_build(&root)?;
        pack_rom(&root)?;
        Ok(())
    };
    match steps() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[n64] {e}");
            ExitCode::FAILURE
        }
    }
}

fn ares_exe(root: &Path) -> Option<PathBuf> {
    let candidates = [root.join("ares-v148/ares.exe"), root.join("ares-v148/ares")];
    candidates.into_iter().find(|p| p.exists())
}

pub fn run() -> ExitCode {
    let code = build(false);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let root = repo_root();
    let Some(ares) = ares_exe(&root) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };
    println!("[n64] launching ares...");
    match Command::new(&ares)
        .arg(root.join("target/n64/trino.z64"))
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[n64] failed to launch ares: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Console live-reload loop: watch game source + assets, rebuild the ROM on
/// change and relaunch ares. No in-place hot reload on the N64 — kill +
/// relaunch IS the loop (n64.mk-style dev cycle).
pub fn watch() -> ExitCode {
    use std::sync::mpsc;
    use std::time::Duration;

    use notify_debouncer_full::notify::RecursiveMode;
    use notify_debouncer_full::{DebounceEventResult, new_debouncer};

    let root = repo_root();
    let Some(ares) = ares_exe(&root) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };
    if build(false) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    fn launch(ares: &Path, root: &Path) -> std::io::Result<std::process::Child> {
        Command::new(ares)
            .arg(root.join("target/n64/trino.z64"))
            .spawn()
    }
    let mut child = match launch(&ares, &root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[n64] failed to launch ares: {e}");
            return ExitCode::FAILURE;
        }
    };

    let (tx, rx) = mpsc::channel();
    let mut debouncer = match new_debouncer(
        Duration::from_millis(300),
        None,
        move |result: DebounceEventResult| {
            if let Ok(events) = result
                && !events.is_empty()
            {
                let _ = tx.send(());
            }
        },
    ) {
        Ok(d) => d,
        Err(e) => {
            eprintln!("[n64] failed to start watcher: {e}");
            let _ = child.kill();
            return ExitCode::FAILURE;
        }
    };
    for dir in ["examples", "assets", "apps/n64", "crates/platform-n64"] {
        if let Err(e) = debouncer.watch(root.join(dir), RecursiveMode::Recursive) {
            eprintln!("[n64] failed to watch {dir}/: {e}");
            let _ = child.kill();
            return ExitCode::FAILURE;
        }
    }

    println!("[n64] watching examples/, assets/, apps/n64/, crates/platform-n64/ — Ctrl+C to stop");
    loop {
        if rx.recv().is_err() {
            let _ = child.kill();
            return ExitCode::SUCCESS;
        }
        while rx.try_recv().is_ok() {}
        println!("[n64] change detected, rebuilding ROM...");
        if build(false) == ExitCode::SUCCESS {
            let _ = child.kill();
            let _ = child.wait();
            match launch(&ares, &root) {
                Ok(c) => child = c,
                Err(e) => {
                    eprintln!("[n64] failed to relaunch ares: {e}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            eprintln!("[n64] build failed — keeping the previous ROM running (fix and save again)");
        }
    }
}

/// Boot the test ROM in ares and watch stdout for the ISViewer magic
/// strings. Kills the emulator on verdict or timeout.
pub fn test() -> ExitCode {
    let code = build(true);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let root = repo_root();
    let Some(ares) = ares_exe(&root) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };

    println!("[n64] booting test ROM in ares (60s timeout)...");
    let mut child = match Command::new(&ares)
        .arg(root.join("target/n64/trino.z64"))
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[n64] failed to launch ares: {e}");
            return ExitCode::FAILURE;
        }
    };

    use std::io::BufRead;
    let stdout = child.stdout.take().expect("piped stdout");
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx2 = tx.clone();
    let stderr = child.stderr.take().expect("piped stderr");
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stdout)
            .lines()
            .map_while(Result::ok)
        {
            let _ = tx.send(line);
        }
    });
    std::thread::spawn(move || {
        for line in std::io::BufReader::new(stderr)
            .lines()
            .map_while(Result::ok)
        {
            let _ = tx2.send(line);
        }
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    let verdict = loop {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break Err("timeout: no TRINO_TEST_* output within 60s".to_string());
        }
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(line) => {
                println!("[ares] {line}");
                if line.contains("TRINO_TEST_PASS") {
                    break Ok(());
                }
                if let Some(idx) = line.find("TRINO_TEST_FAIL") {
                    break Err(line[idx..].to_string());
                }
            }
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                break Err("ares exited before reporting a verdict".to_string());
            }
        }
    };
    let _ = child.kill();

    match verdict {
        Ok(()) => {
            println!("[n64] TEST PASS");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[n64] TEST FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}
