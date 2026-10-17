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

use crate::project::Project;

const IMAGE: &str = "trino-n64";
const TARGET_JSON: &str = "platforms/n64/mips-nintendo64-none.json";

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

/// Run a shell script inside the toolchain image. The engine is mounted at
/// `/engine` and the project at `/project` (the same directory for the
/// engine's own demo); the script runs in `/project` when `in_project`,
/// else in `/engine` (the shim and RSP objects belong to the engine). The
/// script goes through a file, never through the command line: on Windows,
/// wsl.exe re-interprets arguments with its own shell, which would expand
/// `$VARS` outside the container.
fn docker_sh(
    p: &Project,
    in_project: bool,
    script: &str,
) -> std::io::Result<std::process::ExitStatus> {
    let root = if in_project { &p.root } else { &p.engine };
    let workdir = if in_project { "/project" } else { "/engine" };
    let script_dir = root.join("target/n64");
    std::fs::create_dir_all(&script_dir)?;
    // \n only: bash inside the container chokes on \r.
    std::fs::write(
        script_dir.join(".docker-step.sh"),
        script.replace("\r\n", "\n"),
    )?;

    let engine_vol = format!("{}:/engine", docker_volume_path(&p.engine));
    let project_vol = format!("{}:/project", docker_volume_path(&p.root));
    let script_path = format!("{workdir}/target/n64/.docker-step.sh");
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
        &engine_vol,
        "-v",
        &project_vol,
        "-w",
        workdir,
        IMAGE,
        "bash",
        &script_path,
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

/// `true` when `obj` exists and is newer than every source.
fn up_to_date(obj: &Path, sources: &[std::path::PathBuf]) -> bool {
    let time = |p: &Path| p.metadata().and_then(|m| m.modified()).ok();
    let Some(obj_time) = time(obj) else {
        return false;
    };
    sources
        .iter()
        .all(|s| time(s).is_some_and(|t| t <= obj_time))
}

/// Compile the C shim inside the image (real libdragon headers + o64 ABI),
/// and the RSP overlay (`rsp_trino.S`) the way libdragon's `n64.mk` builds
/// RSP ucode: assemble against `rsp.ld`, cut `.text`/`.data` to binaries,
/// wrap each in an object exposing `rsp_trino_{text,data}_{start,end}`
/// (what `DEFINE_RSP_UCODE` expects).
fn compile_shim(p: &Project) -> Result<(), String> {
    let root = &p.engine;
    let shim_dir = root.join("crates/platform-n64/shim");
    let shim_src = shim_dir.join("trino_shim.c");
    let rsp_src = shim_dir.join("rsp_trino.S");
    if !up_to_date(&root.join("target/n64/shim.o"), &[shim_src])
        || !up_to_date(&root.join("target/n64/rsp_trino.o"), &[rsp_src])
    {
        println!("[n64] compiling C shim + RSP overlay...");
        let script = "set -e\n\
            mkdir -p target/n64/rsp\n\
            mips64-elf-gcc -c -std=gnu17 -march=vr4300 -mtune=vr4300 -mabi=o64 -O2 -G0 \
            -ffunction-sections -fdata-sections -DN64 \
            -I$N64_INST/mips64-elf/include \
            crates/platform-n64/shim/trino_shim.c -o target/n64/shim.o\n\
            cd target/n64/rsp\n\
            mips64-elf-gcc -march=mips1 -mabi=32 -Wa,--fatal-warnings \
            -I$N64_INST/mips64-elf/include -L$N64_INST/mips64-elf/lib -nostartfiles \
            -Wl,-Trsp.ld -Wl,--gc-sections -Wl,-Map=rsp_trino.map \
            -o rsp_trino.elf ../../../crates/platform-n64/shim/rsp_trino.S\n\
            mips64-elf-size -G rsp_trino.elf\n\
            mips64-elf-objcopy -O binary -j .text rsp_trino.elf rsp_trino.text.bin\n\
            mips64-elf-objcopy -O binary -j .data rsp_trino.elf rsp_trino.data.bin\n\
            for s in text data; do mips64-elf-objcopy -I binary -O elf32-bigmips -B mips4300 \
              --redefine-sym _binary_rsp_trino_${s}_bin_start=rsp_trino_${s}_start \
              --redefine-sym _binary_rsp_trino_${s}_bin_end=rsp_trino_${s}_end \
              --redefine-sym _binary_rsp_trino_${s}_bin_size=rsp_trino_${s}_size \
              --set-section-alignment .data=8 --rename-section .text=.data \
              rsp_trino.${s}.bin rsp_trino.${s}.o; done\n\
            mips64-elf-ld -relocatable rsp_trino.text.o rsp_trino.data.o -o ../rsp_trino.o";
        let status = docker_sh(p, false, script).map_err(|e| format!("docker: {e}"))?;
        if !status.success() {
            return Err("shim / RSP overlay compilation failed".into());
        }
    }
    Ok(())
}

/// Bake N64 assets with the image's mksprite/audioconv64 into
/// target/n64/filesystem, plus the index.tsv the runtime reads at boot.
pub fn bake_assets(p: &Project, test_mode: bool) -> Result<(), String> {
    ensure_image(&p.engine)?;
    let root = &p.root;
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
        let staged = stage.join(format!("{id:08x}.wav"));
        stage_wav(&source, &staged, decl.sound_loops())?;
        // Looping voices (ambient sources) need the loop in the wav64.
        let looping = if decl.sound_loops() {
            "--wav-loop true "
        } else {
            ""
        };
        script.push_str(&format!(
            "audioconv64 {looping}-o target/n64/filesystem target/n64/stage/{id:08x}.wav\n"
        ));
        index.push_str(&format!("{id:08x}\tsound\t{id:08x}.wav64\n"));
    }
    for (name, decl) in &manifest.music {
        let logical = format!("music/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        stage_wav(
            &source,
            &stage.join(format!("{id:08x}.wav")),
            decl.music_loops(),
        )?;
        // Loop metadata is baked into the wav64 on the N64; the runtime's
        // `looped` flag is a no-op there (see the shim). Stingers declare
        // `loop = false`.
        let looping = if decl.music_loops() { "true" } else { "false" };
        script.push_str(&format!(
            "audioconv64 --wav-loop {looping} -o target/n64/filesystem target/n64/stage/{id:08x}.wav\n"
        ));
        index.push_str(&format!("{id:08x}\tmusic\t{id:08x}.wav64\n"));
    }
    for (name, decl) in &manifest.models {
        if !decl.bakes_for(Platform::N64) {
            continue;
        }
        let logical = format!("models/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        // TMDL is portable: same blob on every platform, no container tool.
        let blob = trino_asset_pipeline::bake_model(&source, &decl.options())?;
        std::fs::write(fsdir.join(format!("{id:08x}.tmdl")), blob).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tmodel\t{id:08x}.tmdl\n"));
    }

    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        let id = trino_core::asset_id(&logical);
        let format = decl.format_for(&logical, Platform::N64)?;
        let mksprite_format = match format.as_str() {
            "RGBA5551" => "RGBA16",
            other => other,
        };
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        let font = trino_asset_pipeline::bake_font(&source, decl)
            .map_err(|e| format!("font `{logical}`: {e}"))?;
        // Atlas -> sprite (mksprite, same handle); table -> verbatim TFNT.
        // Intensity formats take alpha from the color: stage the coverage
        // as the intensity, so antialiased edges stay soft.
        let rgba = if matches!(format.as_str(), "I4" | "I8") {
            intensity_from_alpha(&font.rgba)
        } else {
            font.rgba.clone()
        };
        let png = trino_asset_pipeline::font::encode_png(font.width, font.height, &rgba)?;
        std::fs::write(stage.join(format!("{id:08x}.png")), png).map_err(|e| e.to_string())?;
        script.push_str(&format!(
            "mksprite --format {mksprite_format} -o target/n64/filesystem target/n64/stage/{id:08x}.png\n"
        ));
        std::fs::write(fsdir.join(format!("{id:08x}.font")), &font.table)
            .map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tsprite\t{id:08x}.sprite\n"));
        index.push_str(&format!("{id:08x}\tfont\t{id:08x}.font\n"));
    }
    for (name, decl) in &manifest.blobs {
        let logical = format!("blobs/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        std::fs::copy(&source, fsdir.join(format!("{id:08x}.blob"))).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tblob\t{id:08x}.blob\n"));
    }
    for (name, decl) in &manifest.particles {
        let logical = format!("particles/{name}");
        let id = trino_core::asset_id(&logical);
        // This platform's cap (0 = the effect is off here).
        let source = trino_asset_pipeline::resolve_source(
            &root.join("assets"),
            trino_asset_pipeline::Platform::N64,
            &decl.file,
        )?;
        let blob =
            trino_asset_pipeline::particles::bake(&source, trino_asset_pipeline::Platform::N64)?;
        std::fs::write(fsdir.join(format!("{id:08x}.blob")), blob).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tblob\t{id:08x}.blob\n"));
    }
    for (name, decl) in &manifest.scenes {
        let logical = format!("scenes/{name}");
        let id = trino_core::asset_id(&logical);
        // TSCN is portable: the same blob on every platform.
        let blob = trino_asset_pipeline::bake_scene_for(
            &root.join("assets"),
            &decl.file,
            trino_asset_pipeline::Platform::N64,
        )?;
        std::fs::write(fsdir.join(format!("{id:08x}.blob")), blob).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tblob\t{id:08x}.blob\n"));
    }

    std::fs::write(fsdir.join("index.tsv"), index).map_err(|e| e.to_string())?;
    if test_mode {
        std::fs::write(fsdir.join("test_mode"), "1").map_err(|e| e.to_string())?;
    }

    println!("[n64] baking assets in container...");
    let status = docker_sh(p, true, &script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("asset baking failed".into());
    }
    Ok(())
}

/// Stage a wav for audioconv64: copied as-is, or — when it loops — with
/// its frame count trimmed to a multiple of 16 (libdragon asserts the wav64
/// loop length is; the few dropped samples are inaudible).
fn stage_wav(source: &Path, out: &Path, looped: bool) -> Result<(), String> {
    if !looped {
        return std::fs::copy(source, out)
            .map(|_| ())
            .map_err(|e| format!("{}: {e}", source.display()));
    }
    let mut reader =
        hound::WavReader::open(source).map_err(|e| format!("{}: {e}", source.display()))?;
    let spec = reader.spec();
    let samples: Vec<i32> = reader
        .samples::<i32>()
        .collect::<Result<_, _>>()
        .map_err(|e| format!("{}: {e}", source.display()))?;
    let ch = spec.channels as usize;
    let frames = samples.len() / ch;
    let keep = frames / 16 * 16;
    if keep == 0 {
        return Err(format!("{}: too short to loop", source.display()));
    }
    let mut w = hound::WavWriter::create(out, spec).map_err(|e| e.to_string())?;
    for s in &samples[..keep * ch] {
        w.write_sample(*s).map_err(|e| e.to_string())?;
    }
    w.finalize().map_err(|e| e.to_string())
}

fn cargo_build(p: &Project) -> Result<(), String> {
    println!("[n64] building Rust ({}, build-std)...", p.nightly);
    let target = p.engine.join(TARGET_JSON);
    let status = Command::new("cargo")
        .args([
            &format!("+{}", p.nightly),
            "build",
            "--release",
            "-Zbuild-std=core,alloc",
            "-Zjson-target-spec",
            "--target",
        ])
        .arg(&target)
        .args(["-p", &p.n64_package])
        .current_dir(&p.root)
        .env_remove("CARGO")
        .env_remove("RUSTC")
        .status()
        .map_err(|e| format!("cargo: {e}"))?;
    if !status.success() {
        return Err("N64 Rust build failed".into());
    }
    Ok(())
}

/// White-mask RGBA (a font atlas) as an intensity image for I4/I8: each
/// texel's gray level and alpha are its coverage. mksprite then derives the
/// same alpha the PC and 3DS draw with.
fn intensity_from_alpha(rgba: &[u8]) -> Vec<u8> {
    rgba.chunks(4)
        .flat_map(|p| [p[3], p[3], p[3], p[3]])
        .collect()
}

/// In-container link (mirroring n64.mk's %.elf recipe, plus
/// --no-warn-mismatch for the o32 staticlib + o64 libdragon mix), then
/// n64sym + strip + n64elfcompress + mkdfs + n64tool, mirroring the %.z64
/// recipe.
///
/// `-u mem* -Wl,-lc` ahead of the staticlib (`-Wl,`: the gcc driver drops
/// a repeated plain `-lc`): Rust's `compiler_builtins` defines
/// `memcpy`/`memset`/`memmove`/`memcmp` as *weak* generic loops, and once
/// its object is linked the linker never pulls newlib's strong, hand-tuned
/// MIPS versions — every struct copy in the ROM (libdragon's included) then
/// ran ~5x slower. Undefining them first makes the first `-lc` scan supply
/// newlib's; [`check_mem_symbols`] fails the build if the map says
/// otherwise.
/// `TRINO_N64_TEXT_PAD=<bytes>`: a layout experiment. Puts that many bytes
/// of padding in `.text` ahead of all Rust code, so every function moves
/// while the code stays the same. The N64's 16 KB direct-mapped I-cache
/// makes frame times depend on where hot functions land (the
/// `n64-performance` skill), and a few padded builds tell a real change from
/// a lucky or unlucky layout. Returns the link argument and the build step
/// (both empty when unset or 0).
fn text_pad_step() -> (String, String) {
    let pad = std::env::var("TRINO_N64_TEXT_PAD")
        .ok()
        .and_then(|v| v.trim().parse::<u32>().ok())
        .unwrap_or(0);
    text_pad_for(pad)
}

fn text_pad_for(pad: u32) -> (String, String) {
    if pad == 0 {
        return (String::new(), String::new());
    }
    // Whole instructions, so what follows keeps its alignment class.
    let pad = pad.div_ceil(8) * 8;
    println!("[n64] layout experiment: {pad} bytes of .text padding before the Rust code");
    (
        "target/n64/text_pad.o -Wl,-u,trino_text_pad".into(),
        format!(
            "printf '.section .text.trino_text_pad,\"ax\",@progbits\\n.globl trino_text_pad\\ntrino_text_pad:\\n.space {pad}\\n' > target/n64/text_pad.S\n\
             mips64-elf-gcc -c -march=vr4300 -mabi=o64 target/n64/text_pad.S -o target/n64/text_pad.o\n"
        ),
    )
}

fn pack_rom(p: &Project) -> Result<(), String> {
    println!("[n64] linking + packing ROM...");
    let (name, title, save) = (&p.name, &p.title, &p.n64_save);
    let staticlib = p.n64_staticlib();
    let (pad_obj, pad_step) = text_pad_step();
    let script = format!(
        "set -e\n\
         {pad_step}\
         mips64-elf-g++ -o target/n64/{name}.elf /engine/target/n64/shim.o /engine/target/n64/rsp_trino.o {pad_obj} \
         -Wl,-u,memcpy -Wl,-u,memset -Wl,-u,memmove -Wl,-u,memcmp -Wl,-lc \
         {staticlib} -lc -mabi=o64 -Wl,-g -Wl,-L$N64_INST/mips64-elf/lib \
         -Wl,-ldragon -Wl,-lm -Wl,-ldragonsys -Wl,-Tn64.ld \
         -Wl,--gc-sections -Wl,--no-warn-mismatch \
         -Wl,--wrap -Wl,__do_global_ctors \
         -Wl,-Map=target/n64/{name}.map\n\
         mips64-elf-size -G target/n64/{name}.elf\n\
         cd target/n64\n\
         n64sym {name}.elf {name}.elf.sym\n\
         cp {name}.elf {name}.elf.stripped\n\
         mips64-elf-strip -s {name}.elf.stripped\n\
         n64elfcompress -o . -c 1 {name}.elf.stripped\n\
         mkdfs {name}.dfs filesystem >/dev/null\n\
         n64tool --title '{title}' --toc --output {name}.z64 --align 256 {name}.elf.stripped \
         --align 8 {name}.elf.sym --align 16 {name}.dfs\n\
         ed64romconfig --savetype {save} {name}.z64\n"
    );
    let status = docker_sh(p, true, &script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("ROM packing failed".into());
    }
    let map_path = p.root.join(format!("target/n64/{name}.map"));
    let map =
        std::fs::read_to_string(&map_path).map_err(|e| format!("{}: {e}", map_path.display()))?;
    check_mem_symbols(&map)?;
    println!("[n64] ROM: {}", p.rom().display());
    Ok(())
}

/// The linker map must place `memcpy`/`memset`/`memmove`/`memcmp` from
/// newlib (`libc.a`), never from Rust's `compiler_builtins` (see
/// [`pack_rom`]).
fn check_mem_symbols(map: &str) -> Result<(), String> {
    // Only the memory map counts ("Discarded input sections" lists the
    // losing weak copies too).
    let map = map
        .split_once("Linker script and memory map")
        .map_or(map, |(_, m)| m);
    for sym in ["memcpy", "memset", "memmove", "memcmp"] {
        let section = format!(".text.{sym}");
        let mut lines = map.lines();
        while let Some(line) = lines.next() {
            let t = line.trim_start();
            if !(t == section || t.starts_with(&format!("{section} "))) {
                continue;
            }
            // The input file is on this line or wraps to the next one.
            let file = if t.contains("0x") {
                line.to_string()
            } else {
                lines.next().unwrap_or_default().to_string()
            };
            if file.contains("compiler_builtins") {
                return Err(format!(
                    "{sym} links from Rust's compiler_builtins (slow generic loop), \
                     not newlib — keep `-u {sym} -Wl,-lc` ahead of the staticlib"
                ));
            }
        }
    }
    Ok(())
}

pub fn build(p: &Project, test_mode: bool) -> ExitCode {
    let steps = || -> Result<(), String> {
        ensure_image(&p.engine)?;
        compile_shim(p)?;
        bake_assets(p, test_mode)?;
        cargo_build(p)?;
        pack_rom(p)?;
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

/// ares executable: `$TRINO_ARES` (a path, or `docker` for the
/// `trino-ares` image — see `cargo xtask emulators setup ares`), else
/// `ares-v148/` at the project or engine root.
fn ares_exe(p: &Project) -> Option<PathBuf> {
    if let Ok(env) = std::env::var("TRINO_ARES") {
        let p = PathBuf::from(env);
        return (p.as_os_str() == "docker" || p.exists()).then_some(p);
    }
    [&p.root, &p.engine]
        .iter()
        .flat_map(|r| [r.join("ares-v148/ares.exe"), r.join("ares-v148/ares")])
        .find(|c| c.exists())
}

/// Command that boots `rom` in ares with `args` (native or in Docker).
/// Returns the command and, for Docker, the container name to remove.
fn ares_command(ares: &Path, rom: &Path, args: &[&str]) -> (Command, Option<String>) {
    if ares.as_os_str() != "docker" {
        let mut cmd = Command::new(ares);
        cmd.args(args).arg(rom);
        return (cmd, None);
    }
    let name = format!("trino-ares-{}", std::process::id());
    let dir = rom.parent().expect("rom in a directory");
    let file = rom.file_name().expect("rom file name").to_string_lossy();
    let mut cmd = Command::new("docker");
    cmd.args(["run", "--rm", "--init", "--name", &name, "-v"])
        .arg(format!("{}:/rom:ro", dir.display()))
        .arg(crate::emulators::ARES_IMAGE)
        .args(args)
        // Saves (EEPROM) go to a scratch dir inside the container: a
        // read-only save path pops a modal dialog that blocks the boot.
        .args([
            "--setting",
            "Audio/Driver=None",
            "--setting",
            "Paths/Saves=/tmp/",
        ])
        .arg(format!("/rom/{file}"));
    (cmd, Some(name))
}

pub fn run(p: &Project) -> ExitCode {
    let code = build(p, false);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let Some(ares) = ares_exe(p) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };
    println!("[n64] launching ares...");
    match Command::new(&ares).arg(p.rom()).spawn() {
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
pub fn watch(p: &Project) -> ExitCode {
    use std::sync::mpsc;
    use std::time::Duration;

    use notify_debouncer_full::notify::RecursiveMode;
    use notify_debouncer_full::{DebounceEventResult, new_debouncer};

    let Some(ares) = ares_exe(p) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };
    if build(p, false) != ExitCode::SUCCESS {
        return ExitCode::FAILURE;
    }

    fn launch(ares: &Path, rom: &Path) -> std::io::Result<std::process::Child> {
        Command::new(ares).arg(rom).spawn()
    }
    let rom = p.rom();
    let mut child = match launch(&ares, &rom) {
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
                && crate::real_change(&events)
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
    for dir in &p.watch {
        if let Err(e) = debouncer.watch(p.root.join(dir), RecursiveMode::Recursive) {
            eprintln!("[n64] failed to watch {dir}/: {e}");
            let _ = child.kill();
            return ExitCode::FAILURE;
        }
    }

    println!("[n64] watching {} — Ctrl+C to stop", p.watch.join(", "));
    loop {
        if rx.recv().is_err() {
            let _ = child.kill();
            return ExitCode::SUCCESS;
        }
        while rx.try_recv().is_ok() {}
        println!("[n64] change detected, rebuilding ROM...");
        if build(p, false) == ExitCode::SUCCESS {
            let _ = child.kill();
            let _ = child.wait();
            match launch(&ares, &rom) {
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
pub fn test(p: &Project, bless: bool) -> ExitCode {
    let code = build(p, true);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let root = &p.root;
    let Some(ares) = ares_exe(p) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };

    let timeout = crate::emulators::timeout_secs();
    let idle = crate::emulators::idle_secs();
    println!("[n64] booting test ROM in ares ({timeout}s timeout, {idle}s idle limit)...");
    // Native-resolution RDP: the golden frame is compared pixel-exactly.
    let (mut cmd, container) = ares_command(
        &ares,
        &p.rom(),
        &["--no-file-prompt", "--setting", "Video/Quality=SD"],
    );
    let mut child = match cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn() {
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

    let mut watchdog = crate::emulators::Watchdog::new();
    let mut fb = crate::fbgolden::FbCapture::default();
    let mut frames: Vec<crate::fbgolden::FbCapture> = Vec::new();
    let mut perf_scenes = Vec::new();
    let mut micro_scenes = Vec::new();
    let mut frame_lines = Vec::new();
    let verdict = loop {
        if let Some(why) = watchdog.expired(Instant::now()) {
            break Err(why);
        }
        let left = watchdog.left(Instant::now());
        match rx.recv_timeout(left.min(Duration::from_millis(500))) {
            Ok(line) => {
                watchdog.saw(&line, Instant::now());
                // Framebuffer dump rows are collected, not echoed; each
                // TRINO_FB_END closes one image (they arrive in order).
                if fb.feed(&line) {
                    if fb.done() {
                        frames.push(std::mem::take(&mut fb));
                    }
                    continue;
                }
                println!("[ares] {line}");
                if let Some(scene) = crate::perf::SceneLine::parse(&line) {
                    // Micro scenes measure fixed costs: reported, never
                    // gated or blessed.
                    if crate::perf::is_micro(&scene.name) {
                        micro_scenes.push(scene);
                    } else {
                        perf_scenes.push(scene);
                    }
                }
                if let Some(f) = parse_frame_line(&line) {
                    frame_lines.push(f);
                }
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
    if let Some(name) = container {
        let _ = Command::new("docker")
            .args(["rm", "-f", &name])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }

    let verdict = if p.is_engine {
        verdict.and_then(|()| engine_gates(root, &frames, &perf_scenes, &micro_scenes, bless))
    } else {
        verdict.and_then(|()| {
            project_gates(p, "n64", &p.test.n64_goldens, &frames, bless, None)?;
            frame_gate(&frame_lines, p.test.n64_max_frame_us)
        })
    };

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

/// castle64's gates: the emulator goldens and the perf gates.
fn engine_gates(
    root: &Path,
    frames: &[crate::fbgolden::FbCapture],
    perf_scenes: &[crate::perf::SceneLine],
    micro_scenes: &[crate::perf::SceneLine],
    bless: bool,
) -> Result<(), String> {
    // Emulator golden images, in dump order: the showcase's first frame
    // (fogged: the CPU T&L path) and castle64's hub (the RSP T&L path).
    for (n, name) in ["showcase", "castle64_hub"].iter().enumerate() {
        let img = frames
            .get(n)
            .ok_or_else(|| format!("golden frame {name}: not dumped"))?
            .image()
            .map_err(|e| format!("golden frame {name}: {e}"))?;
        let msg = crate::fbgolden::check(root, "n64", name, img, bless)?;
        println!("[n64] golden frame: {msg}");
    }
    // Performance gates: same T&L work as the PC baseline, frame costs
    // under the committed ceilings.
    let table =
        crate::perf::gate(root, perf_scenes, bless).map_err(|e| format!("perf gate:\n{e}"))?;
    println!("[n64] perf (emulated us per frame):\n{table}");
    let ab = crate::perf::rsp_ab_report(perf_scenes, micro_scenes);
    if !ab.is_empty() {
        println!("[n64] castle64, RSP T&L vs CPU T&L (same ROM, emulated us per frame):\n{ab}");
    }
    let table =
        crate::perf::micro_report(micro_scenes).map_err(|e| format!("micro perf scenes:\n{e}"))?;
    println!("[n64] T&L fixed costs (micro scenes, ungated):\n{table}");
    Ok(())
}

/// An external project's goldens: the `TRINO_FB` dumps, in order, against
/// `<project>/tests/golden/<platform>/<name>.png` — exact on the N64
/// (`tolerance: None`), within `(mean, outliers)` on the 3DS.
pub fn project_gates(
    p: &Project,
    platform: &str,
    names: &[String],
    frames: &[crate::fbgolden::FbCapture],
    bless: bool,
    tolerance: Option<(f32, f32)>,
) -> Result<(), String> {
    for (n, name) in names.iter().enumerate() {
        let img = frames
            .get(n)
            .ok_or_else(|| format!("golden frame {name}: not dumped"))?
            .image()
            .map_err(|e| format!("golden frame {name}: {e}"))?;
        let msg = match tolerance {
            None => crate::fbgolden::check(&p.root, platform, name, img, bless)?,
            Some((mean, outliers)) => {
                crate::fbgolden::check_close(&p.root, platform, name, img, bless, mean, outliers)?
            }
        };
        println!("[{platform}] golden frame {name}: {msg}");
    }
    Ok(())
}

/// `TRINO_FRAME:<scene> avg_us=<n> max_us=<n> frames=<n>` — a project
/// self-test's frame-time report (emulated console time). Returns
/// `(scene, avg_us, max_us)`.
pub fn parse_frame_line(line: &str) -> Option<(String, u32, u32)> {
    let rest = &line[line.find("TRINO_FRAME:")? + "TRINO_FRAME:".len()..];
    let mut parts = rest.split_whitespace();
    let scene = parts.next()?.to_string();
    let (mut avg, mut max) = (None, None);
    for kv in parts {
        match kv.split_once('=') {
            Some(("avg_us", v)) => avg = v.parse().ok(),
            Some(("max_us", v)) => max = v.parse().ok(),
            _ => {}
        }
    }
    Some((scene, avg?, max?))
}

/// Gate the project's frame-time lines on `max_frame_us` (the average per
/// scene; the worst frame is reported, not gated: one-off loads spike).
fn frame_gate(lines: &[(String, u32, u32)], max_frame_us: Option<u32>) -> Result<(), String> {
    if lines.is_empty() {
        return match max_frame_us {
            Some(_) => {
                Err("n64_max_frame_us is set but the ROM printed no TRINO_FRAME line".into())
            }
            None => Ok(()),
        };
    }
    let mut table = String::from("scene                      avg ms   max ms    fps\n");
    let mut over = Vec::new();
    for (scene, avg, max) in lines {
        table.push_str(&format!(
            "{scene:<24} {:>8.2} {:>8.2} {:>6.1}\n",
            *avg as f32 / 1000.0,
            *max as f32 / 1000.0,
            1_000_000.0 / (*avg).max(1) as f32
        ));
        if let Some(limit) = max_frame_us
            && *avg > limit
        {
            over.push(format!("{scene}: {avg} us > {limit} us"));
        }
    }
    println!("[n64] frame time (emulated):\n{table}");
    if over.is_empty() {
        Ok(())
    } else {
        Err(format!("frame budget exceeded: {}", over.join("; ")))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_text_pad_is_whole_instructions_before_the_rust_code() {
        assert_eq!(text_pad_for(0), (String::new(), String::new()));
        let (obj, step) = text_pad_for(1000);
        assert_eq!(obj, "target/n64/text_pad.o -Wl,-u,trino_text_pad");
        assert!(step.contains(".space 1000\\n"), "{step}");
        // Rounded up to 8 bytes: what follows keeps its alignment class.
        let (_, step) = text_pad_for(13);
        assert!(step.contains(".space 16\\n"), "{step}");
        assert!(step.contains("mips64-elf-gcc -c"), "{step}");
    }

    #[test]
    fn intensity_atlases_carry_coverage_in_their_gray_level() {
        let rgba = [255, 255, 255, 128, 0, 0, 0, 0, 255, 255, 255, 255];
        assert_eq!(
            intensity_from_alpha(&rgba),
            vec![128, 128, 128, 128, 0, 0, 0, 0, 255, 255, 255, 255]
        );
    }

    #[test]
    fn frame_lines_parse_and_gate() {
        let l =
            parse_frame_line("[ares] TRINO_FRAME:hall-busy avg_us=21000 max_us=30500 frames=120");
        assert_eq!(l, Some(("hall-busy".into(), 21000, 30500)));
        assert_eq!(parse_frame_line("TRINO_FRAME:x avg_us=1"), None);
        let lines = vec![l.unwrap()];
        assert!(frame_gate(&lines, Some(33333)).is_ok());
        assert!(frame_gate(&lines, Some(20000)).is_err());
        assert!(frame_gate(&[], Some(33333)).is_err());
        assert!(frame_gate(&[], None).is_ok());
    }

    fn write_wav(path: &Path, channels: u16, frames: usize) {
        let spec = hound::WavSpec {
            channels,
            sample_rate: 22_050,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let mut w = hound::WavWriter::create(path, spec).unwrap();
        for i in 0..frames * channels as usize {
            w.write_sample(i as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    #[test]
    fn looped_wavs_are_trimmed_to_whole_16_frame_blocks() {
        let dir = tempfile::tempdir().unwrap();
        let (src, out) = (dir.path().join("in.wav"), dir.path().join("out.wav"));
        write_wav(&src, 2, 37);
        stage_wav(&src, &out, true).unwrap();
        let mut r = hound::WavReader::open(&out).unwrap();
        assert_eq!(r.spec().channels, 2);
        let samples: Vec<i16> = r.samples::<i16>().map(Result::unwrap).collect();
        assert_eq!(samples.len(), 32 * 2, "37 frames trim to 32");
        assert_eq!(samples[..4], [0, 1, 2, 3], "the head is kept as is");
        // One-shots are copied byte for byte.
        stage_wav(&src, &out, false).unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), std::fs::read(&out).unwrap());
        // Under 16 frames there is nothing to loop.
        write_wav(&src, 1, 15);
        assert!(
            stage_wav(&src, &out, true)
                .unwrap_err()
                .contains("too short")
        );
        assert!(stage_wav(&dir.path().join("missing.wav"), &out, true).is_err());
    }

    #[test]
    fn objects_are_up_to_date_only_when_newer_than_every_source() {
        let dir = tempfile::tempdir().unwrap();
        let (src, obj) = (dir.path().join("a.c"), dir.path().join("a.o"));
        std::fs::write(&src, "int x;").unwrap();
        assert!(
            !up_to_date(&obj, std::slice::from_ref(&src)),
            "no object yet"
        );
        std::fs::write(&obj, "o").unwrap();
        let old = std::time::SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(1_000);
        std::fs::File::options()
            .write(true)
            .open(&src)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert!(up_to_date(&obj, std::slice::from_ref(&src)));
        assert!(
            !up_to_date(&obj, &[dir.path().join("gone.c")]),
            "a missing source forces a rebuild"
        );
    }

    #[test]
    fn docker_paths_map_windows_drives_to_wsl_mounts() {
        assert_eq!(
            docker_volume_path(Path::new("/home/u/trino")),
            "/home/u/trino"
        );
        assert_eq!(
            docker_volume_path(Path::new(r"C:\src\trino")),
            "/mnt/c/src/trino"
        );
        assert_eq!(
            docker_volume_path(Path::new(r"\\?\D:\work\trino")),
            "/mnt/d/work/trino"
        );
    }

    #[test]
    fn mem_symbols_must_come_from_newlib() {
        let good = "Discarded input sections\n \
                    .text.memcpy   0x00000000       0x1c libtrino.a(compiler_builtins-x.o)\n\
                    Linker script and memory map\n \
                    .text          0x800014a0       0x60 libc.a(libc_a-memcpy.o)\n";
        assert!(check_mem_symbols(good).is_ok());
        let bad = "Linker script and memory map\n \
                   .text.memcpy   0x800230b8       0x1c libtrino.a(compiler_builtins-4d4.o)\n";
        assert!(check_mem_symbols(bad).unwrap_err().contains("memcpy"));
        // Long section names wrap the input file to the next line.
        let wrapped = "Linker script and memory map\n .text.memmove\n                \
                       0x80023000       0x40 libtrino.a(compiler_builtins-9.o)\n";
        assert!(check_mem_symbols(wrapped).unwrap_err().contains("memmove"));
    }
}
