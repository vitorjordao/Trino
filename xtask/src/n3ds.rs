//! 3DS pipeline: local devkitPro (arm-none-eabi-gcc, tex3ds, 3dsxtool) ->
//! host cargo (nightly, build-std, built-in `armv6k-nintendo-3ds` target) ->
//! `.3dsx` -> Azahar.
//!
//! Unlike the N64 there is no Docker step and no ABI hazard: devkitPro is a
//! local install, both sides of the FFI are ARM EABI-hf, and the Rust target
//! ships in rustc (Tier 3). The C shim exists because citro2d is mostly
//! static-inline C — and to keep the two console backends symmetrical.
//!
//! Test protocol: Azahar logs `svcOutputDebugString` output to its log file;
//! the harness tails it for the TRINO_TEST_* magic strings.

use std::path::{Path, PathBuf};
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

use trino_asset_pipeline::{Manifest, Platform, resolve_source};

const TARGET: &str = "armv6k-nintendo-3ds";
const ELF: &str = "target/armv6k-nintendo-3ds/release/trino-3ds.elf";

pub fn repo_root() -> PathBuf {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .canonicalize()
        .expect("repo root");
    // canonicalize() yields \\?\C:\... on Windows; mingw-built devkitPro
    // tools reject the verbatim prefix, so strip it.
    let s = root.to_string_lossy();
    match s.strip_prefix(r"\\?\") {
        Some(rest) => PathBuf::from(rest),
        None => root,
    }
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

/// devkitPro root: $DEVKITPRO when it is a real directory (on Windows the
/// installer registers the msys2-style `/opt/devkitpro`, useless outside
/// msys2), else the platform default install location.
fn devkitpro() -> Result<PathBuf, String> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(env) = std::env::var("DEVKITPRO") {
        candidates.push(PathBuf::from(env));
    }
    candidates.push(PathBuf::from(r"C:\devkitPro"));
    candidates.push(PathBuf::from("/opt/devkitpro"));
    candidates
        .into_iter()
        .find(|p| p.join("libctru").is_dir())
        .ok_or_else(|| {
            "devkitPro (with libctru) not found — install it from \
             https://devkitpro.org (packages: 3ds-dev)"
                .to_string()
        })
}

fn tool(dkp: &Path, rel: &str, name: &str) -> Result<PathBuf, String> {
    let p = dkp.join(rel).join(exe(name));
    if p.exists() {
        Ok(p)
    } else {
        Err(format!("{name} not found at {}", p.display()))
    }
}

/// Compile the C shim with devkitARM's gcc against the installed libctru
/// headers (mtime-cached).
fn compile_shim(root: &Path, dkp: &Path) -> Result<(), String> {
    let shim_src = root.join("crates/platform-3ds/shim/trino_shim_3ds.c");
    let shim_obj = root.join("target/3ds/shim.o");
    if let (Ok(src_meta), Ok(obj_meta)) = (shim_src.metadata(), shim_obj.metadata())
        && let (Ok(src_time), Ok(obj_time)) = (src_meta.modified(), obj_meta.modified())
        && obj_time >= src_time
    {
        return Ok(());
    }
    println!("[3ds] compiling C shim...");
    std::fs::create_dir_all(root.join("target/3ds")).map_err(|e| e.to_string())?;
    let gcc = tool(dkp, "devkitARM/bin", "arm-none-eabi-gcc")?;
    let status = Command::new(&gcc)
        .args([
            "-c",
            "-std=gnu17",
            "-march=armv6k",
            "-mtune=mpcore",
            "-mfloat-abi=hard",
            "-mtp=soft",
            "-O2",
            "-mword-relocations",
            "-ffunction-sections",
            "-fdata-sections",
            "-D__3DS__",
        ])
        .arg(format!("-I{}", dkp.join("libctru/include").display()))
        .arg(root.join("crates/platform-3ds/shim/trino_shim_3ds.c"))
        .arg("-o")
        .arg(&shim_obj)
        .status()
        .map_err(|e| format!("gcc: {e}"))?;
    if !status.success() {
        return Err("shim compilation failed".into());
    }
    Ok(())
}

/// Bake 3DS assets into target/3ds/romfs: tex3ds for sprites, raw PCM16
/// (12-byte header: rate, channels, frames) for sounds, plus the index.tsv
/// the runtime reads at boot.
pub fn bake_assets(root: &Path, test_mode: bool) -> Result<(), String> {
    let dkp = devkitpro()?;
    let tex3ds = tool(&dkp, "tools/bin", "tex3ds")?;
    let manifest = Manifest::load(&root.join("assets/manifest.toml"))?;

    let stage = root.join("target/3ds/stage");
    let romfs = root.join("target/3ds/romfs");
    let _ = std::fs::remove_dir_all(&stage);
    let _ = std::fs::remove_dir_all(&romfs);
    std::fs::create_dir_all(&stage).map_err(|e| e.to_string())?;
    std::fs::create_dir_all(&romfs).map_err(|e| e.to_string())?;

    let mut index = String::new();

    println!("[3ds] baking assets...");
    for (name, decl) in &manifest.sprites {
        let logical = format!("sprites/{name}");
        let id = trino_core::asset_id(&logical);
        let format = decl.format_for(&logical, Platform::N3ds)?;
        // Manifest names follow the hardware format; tex3ds spells them
        // lowercase-compact.
        let tex3ds_format = match format.as_str() {
            "RGBA8" => "rgba8",
            "RGB565" => "rgb565",
            other => return Err(format!("{logical}: no tex3ds mapping for format {other}")),
        };
        let source = resolve_source(&root.join("assets"), Platform::N3ds, &decl.file)?;
        let staged = stage.join(format!("{id:08x}.png"));
        std::fs::copy(&source, &staged).map_err(|e| e.to_string())?;
        let out = romfs.join(format!("{id:08x}.t3x"));
        let status = Command::new(&tex3ds)
            .arg("-f")
            .arg(tex3ds_format)
            .arg("-o")
            .arg(&out)
            .arg(&staged)
            .status()
            .map_err(|e| format!("tex3ds: {e}"))?;
        if !status.success() {
            return Err(format!("tex3ds failed for {logical}"));
        }
        index.push_str(&format!("{id:08x}\tsprite\t{id:08x}.t3x\n"));
    }
    for (name, decl) in &manifest.sounds {
        let logical = format!("sounds/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N3ds, &decl.file)?;
        let out = romfs.join(format!("{id:08x}.pcm16"));
        bake_pcm16(&source, &out)?;
        index.push_str(&format!("{id:08x}\tsound\t{id:08x}.pcm16\n"));
    }
    for (name, decl) in &manifest.music {
        let logical = format!("music/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N3ds, &decl.file)?;
        let out = romfs.join(format!("{id:08x}.pcm16"));
        bake_pcm16(&source, &out)?;
        index.push_str(&format!("{id:08x}\tmusic\t{id:08x}.pcm16\n"));
    }
    for (name, decl) in &manifest.models {
        let logical = format!("models/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N3ds, &decl.file)?;
        // TMDL is portable: same blob on every platform, no container tool.
        let blob = trino_asset_pipeline::bake_model_tmdl(&source)?;
        std::fs::write(romfs.join(format!("{id:08x}.tmdl")), blob).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tmodel\t{id:08x}.tmdl\n"));
    }

    std::fs::write(romfs.join("index.tsv"), index).map_err(|e| e.to_string())?;
    if test_mode {
        std::fs::write(romfs.join("test_mode"), "1").map_err(|e| e.to_string())?;
    }
    Ok(())
}

/// wav -> raw PCM16 LE with a 12-byte header (u32 sample_rate, u32 channels,
/// u32 frames) — the format `trino_wav_load` in the shim reads into linear
/// memory for ndsp.
fn bake_pcm16(wav: &Path, out: &Path) -> Result<(), String> {
    let mut reader = hound::WavReader::open(wav).map_err(|e| format!("{}: {e}", wav.display()))?;
    let spec = reader.spec();
    let samples: Vec<i16> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Int, 16) => reader
            .samples::<i16>()
            .collect::<Result<_, _>>()
            .map_err(|e| e.to_string())?,
        other => {
            return Err(format!(
                "{}: unsupported wav format {other:?} (bake expects 16-bit PCM)",
                wav.display()
            ));
        }
    };
    let frames = samples.len() as u32 / spec.channels as u32;
    let mut bytes = Vec::with_capacity(12 + samples.len() * 2);
    bytes.extend_from_slice(&spec.sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(spec.channels as u32).to_le_bytes());
    bytes.extend_from_slice(&frames.to_le_bytes());
    for s in samples {
        bytes.extend_from_slice(&s.to_le_bytes());
    }
    std::fs::write(out, bytes).map_err(|e| e.to_string())
}

fn cargo_build(root: &Path, dkp: &Path) -> Result<(), String> {
    println!("[3ds] building Rust (nightly, build-std)...");
    // The target's linker is arm-none-eabi-gcc (found via PATH) and its spec
    // already carries -specs=3dsx.specs; we only add the shim + libraries.
    let path = std::env::var("PATH").unwrap_or_default();
    let sep = if cfg!(windows) { ';' } else { ':' };
    let path = format!("{}{sep}{path}", dkp.join("devkitARM/bin").display());

    let shim = root.join("target/3ds/shim.o");
    let rustflags = format!(
        "-Cpanic=abort -Clink-arg={shim} -Clink-arg=-L{libs} \
         -Clink-arg=-lcitro2d -Clink-arg=-lcitro3d -Clink-arg=-lctru -Clink-arg=-lm",
        shim = shim.display(),
        libs = dkp.join("libctru/lib").display(),
    );

    let status = Command::new("cargo")
        .args([
            "+nightly",
            "build",
            "--release",
            "-Zbuild-std=core,alloc",
            "--target",
            TARGET,
            "-p",
            "trino-app-3ds",
        ])
        .current_dir(root)
        .env("PATH", path)
        .env("RUSTFLAGS", rustflags)
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err("3DS Rust build failed".into());
    }
    Ok(())
}

/// smdh metadata + 3dsxtool (ELF + RomFS -> .3dsx).
fn pack_3dsx(root: &Path, dkp: &Path) -> Result<(), String> {
    println!("[3ds] packing .3dsx...");
    let smdhtool = tool(dkp, "tools/bin", "smdhtool")?;
    let tool_3dsx = tool(dkp, "tools/bin", "3dsxtool")?;
    let smdh = root.join("target/3ds/trino.smdh");
    let icon = dkp.join("libctru/default_icon.png");
    let status = Command::new(&smdhtool)
        .args(["--create", "Trino", "Trino hello-sprite", "Trino"])
        .arg(&icon)
        .arg(&smdh)
        .status()
        .map_err(|e| format!("smdhtool: {e}"))?;
    if !status.success() {
        return Err("smdhtool failed".into());
    }
    let status = Command::new(&tool_3dsx)
        .arg(root.join(ELF))
        .arg(root.join("target/3ds/trino.3dsx"))
        .arg(format!("--smdh={}", smdh.display()))
        .arg(format!(
            "--romfs={}",
            root.join("target/3ds/romfs").display()
        ))
        .status()
        .map_err(|e| format!("3dsxtool: {e}"))?;
    if !status.success() {
        return Err("3dsxtool failed".into());
    }
    println!("[3ds] app: target/3ds/trino.3dsx");
    Ok(())
}

pub fn build(test_mode: bool) -> ExitCode {
    let root = repo_root();
    let steps = || -> Result<(), String> {
        let dkp = devkitpro()?;
        compile_shim(&root, &dkp)?;
        bake_assets(&root, test_mode)?;
        cargo_build(&root, &dkp)?;
        pack_3dsx(&root, &dkp)?;
        Ok(())
    };
    match steps() {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[3ds] {e}");
            ExitCode::FAILURE
        }
    }
}

/// Azahar executable: $TRINO_AZAHAR, the default install location, or PATH.
fn azahar_exe() -> Option<PathBuf> {
    if let Ok(env) = std::env::var("TRINO_AZAHAR") {
        let p = PathBuf::from(env);
        if p.exists() {
            return Some(p);
        }
    }
    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        let p = PathBuf::from(local)
            .join("Programs/Azahar")
            .join(exe("azahar"));
        if p.exists() {
            return Some(p);
        }
    }
    which(&exe("azahar"))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(name))
        .find(|p| p.exists())
}

/// Azahar's rotating log file (svcOutputDebugString ends up here).
fn azahar_log() -> Option<PathBuf> {
    let base = if cfg!(windows) {
        PathBuf::from(std::env::var("APPDATA").ok()?)
    } else {
        PathBuf::from(std::env::var("HOME").ok()?).join(".local/share")
    };
    Some(base.join("Azahar/log/azahar_log.txt"))
}

/// svcOutputDebugString logs under `Debug.Emulated` at Debug level, which
/// Azahar's default `*:Info` filter drops — the harness would time out.
/// Idempotently widen the filter in qt-config.ini.
fn ensure_azahar_log_filter() -> Result<(), String> {
    let Some(ini) =
        azahar_log().and_then(|l| Some(l.parent()?.parent()?.join("config/qt-config.ini")))
    else {
        return Ok(()); // no config yet: first run logs everything it needs? no — warn below
    };
    let Ok(content) = std::fs::read_to_string(&ini) else {
        // No config yet (Azahar never ran): the default filter hides the
        // magic strings, so tell the user instead of guessing the format.
        return Err(format!(
            "Azahar config not found at {} — run Azahar once, then re-run the test",
            ini.display()
        ));
    };
    if content.contains("Debug.Emulated") {
        return Ok(());
    }
    let patched = content
        .replace(
            "log_filter\\default=true\r\n",
            "log_filter\\default=false\r\n",
        )
        .replace("log_filter\\default=true\n", "log_filter\\default=false\n")
        .replace(
            "log_filter=*:Info",
            "log_filter=*:Info Debug.Emulated:Trace",
        );
    if patched == content {
        return Err(format!(
            "could not widen Azahar's log_filter in {} — add `Debug.Emulated:Trace` \
             to it manually (Emulation > Configure > Debug)",
            ini.display()
        ));
    }
    println!("[3ds] widening Azahar log filter (Debug.Emulated:Trace) for the test harness");
    std::fs::write(&ini, patched).map_err(|e| e.to_string())
}

pub fn run() -> ExitCode {
    let code = build(false);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let root = repo_root();
    let Some(azahar) = azahar_exe() else {
        eprintln!("[3ds] Azahar not found (set TRINO_AZAHAR to the executable)");
        return ExitCode::FAILURE;
    };
    println!("[3ds] launching Azahar...");
    match Command::new(&azahar)
        .arg(root.join("target/3ds/trino.3dsx"))
        .spawn()
    {
        Ok(_) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("[3ds] failed to launch Azahar: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Boot the test app in Azahar and tail its log file for the magic strings
/// (the GUI build has no console stdout on Windows). Kills the emulator on
/// verdict or timeout.
pub fn test() -> ExitCode {
    let code = build(true);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let root = repo_root();
    let Some(azahar) = azahar_exe() else {
        eprintln!("[3ds] Azahar not found (set TRINO_AZAHAR to the executable)");
        return ExitCode::FAILURE;
    };
    let Some(log) = azahar_log() else {
        eprintln!("[3ds] cannot locate the Azahar log directory");
        return ExitCode::FAILURE;
    };
    if let Err(e) = ensure_azahar_log_filter() {
        eprintln!("[3ds] {e}");
        return ExitCode::FAILURE;
    }
    // Azahar truncates the log per session; remove any stale one so we never
    // match output from a previous run.
    let _ = std::fs::remove_file(&log);

    println!("[3ds] booting test app in Azahar (120s timeout)...");
    let mut child = match Command::new(&azahar)
        .arg(root.join("target/3ds/trino.3dsx"))
        .spawn()
    {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[3ds] failed to launch Azahar: {e}");
            return ExitCode::FAILURE;
        }
    };

    let deadline = Instant::now() + Duration::from_secs(120);
    let mut seen = 0usize;
    let verdict = loop {
        if Instant::now() >= deadline {
            break Err("timeout: no TRINO_TEST_* output within 120s".to_string());
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Err(format!(
                "Azahar exited before reporting a verdict ({status})"
            ));
        }
        std::thread::sleep(Duration::from_millis(500));
        let Ok(content) = std::fs::read_to_string(&log) else {
            continue;
        };
        let fresh = &content[seen.min(content.len())..];
        for line in fresh.lines() {
            if line.contains("TRINO_") {
                println!("[azahar] {line}");
            }
        }
        if fresh.contains("TRINO_TEST_PASS") {
            break Ok(());
        }
        if let Some(idx) = fresh.find("TRINO_TEST_FAIL") {
            let tail: String = fresh[idx..].lines().next().unwrap_or("").to_string();
            break Err(tail);
        }
        seen = content.len();
    };
    let _ = child.kill();

    match verdict {
        Ok(()) => {
            println!("[3ds] TEST PASS");
            ExitCode::SUCCESS
        }
        Err(e) => {
            eprintln!("[3ds] TEST FAIL: {e}");
            ExitCode::FAILURE
        }
    }
}

/// Console live-reload loop: watch game source + assets, rebuild the .3dsx
/// on change and relaunch Azahar (kill + relaunch, like the N64 loop).
pub fn watch() -> ExitCode {
    use std::sync::mpsc;

    use notify_debouncer_full::notify::RecursiveMode;
    use notify_debouncer_full::{DebounceEventResult, new_debouncer};

    let root = repo_root();
    let Some(azahar) = azahar_exe() else {
        eprintln!("[3ds] Azahar not found (set TRINO_AZAHAR to the executable)");
        return ExitCode::FAILURE;
    };
    if build(false) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    fn launch(azahar: &Path, root: &Path) -> std::io::Result<std::process::Child> {
        Command::new(azahar)
            .arg(root.join("target/3ds/trino.3dsx"))
            .spawn()
    }
    let mut child = match launch(&azahar, &root) {
        Ok(c) => c,
        Err(e) => {
            eprintln!("[3ds] failed to launch Azahar: {e}");
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
            eprintln!("[3ds] failed to start watcher: {e}");
            let _ = child.kill();
            return ExitCode::FAILURE;
        }
    };
    for dir in ["examples", "assets", "apps/3ds", "crates/platform-3ds"] {
        if let Err(e) = debouncer.watch(root.join(dir), RecursiveMode::Recursive) {
            eprintln!("[3ds] failed to watch {dir}/: {e}");
            let _ = child.kill();
            return ExitCode::FAILURE;
        }
    }

    println!("[3ds] watching examples/, assets/, apps/3ds/, crates/platform-3ds/ — Ctrl+C to stop");
    loop {
        if rx.recv().is_err() {
            let _ = child.kill();
            return ExitCode::SUCCESS;
        }
        while rx.try_recv().is_ok() {}
        println!("[3ds] change detected, rebuilding...");
        if build(false) == ExitCode::SUCCESS {
            let _ = child.kill();
            let _ = child.wait();
            match launch(&azahar, &root) {
                Ok(c) => child = c,
                Err(e) => {
                    eprintln!("[3ds] failed to relaunch Azahar: {e}");
                    return ExitCode::FAILURE;
                }
            }
        } else {
            eprintln!("[3ds] build failed — keeping the previous app running (fix and save again)");
        }
    }
}
