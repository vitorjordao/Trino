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
/// and the RSP overlays (`rsp_trino.S`, assembled as `rsp_trino` and, with
/// `TRINO_COLORED`, `rsp_trino_color`, and with `TRINO_TEXTURED` too,
/// `rsp_trino_tex`, and with `TRINO_WATER` too, `rsp_trino_water`) the way libdragon's `n64.mk` builds
/// RSP ucode: assemble against `rsp.ld`, cut `.text`/`.data` to binaries,
/// wrap each in an object exposing `<name>_{text,data}_{start,end}` (what
/// `DEFINE_RSP_UCODE` expects).
fn compile_shim(p: &Project) -> Result<(), String> {
    let root = &p.engine;
    let shim_dir = root.join("crates/platform-n64/shim");
    let shim_src = shim_dir.join("trino_shim.c");
    let rsp_src = shim_dir.join("rsp_trino.S");
    let opus_src = shim_dir.join("trino_opus.c");
    if !up_to_date(&root.join("target/n64/shim.o"), &[shim_src])
        || !up_to_date(&root.join("target/n64/rsp_trino.o"), &[rsp_src])
        || !up_to_date(&root.join("target/n64/opus.o"), &[opus_src])
    {
        println!("[n64] compiling C shim + RSP overlay...");
        // The two C files and the four overlays build at the same time,
        // up to the build's thread budget (issue #101): `run` starts a job
        // in the background and waits for the batch once `$J` are running.
        let script = format!(
            "set -e\n\
            J={jobs}; pids=; n=0\n\
            run() {{ \"$@\" & pids=\"$pids $!\"; n=$((n+1)); \
              if [ $n -ge $J ]; then drain; fi; }}\n\
            drain() {{ for p in $pids; do wait $p || exit 1; done; pids=; n=0; }}\n\
            overlay() {{ u=$1; def=; [ $u = rsp_trino_color ] && def=-DTRINO_COLORED\n\
            [ $u = rsp_trino_tex ] && def='-DTRINO_COLORED -DTRINO_TEXTURED'\n\
            [ $u = rsp_trino_water ] && def='-DTRINO_COLORED -DTRINO_TEXTURED -DTRINO_WATER'\n\
            mips64-elf-gcc -march=mips1 -mabi=32 -Wa,--fatal-warnings $def \
            -I$N64_INST/mips64-elf/include -L$N64_INST/mips64-elf/lib -nostartfiles \
            -Wl,-Trsp.ld -Wl,--gc-sections -Wl,-Map=$u.map \
            -o $u.elf ../../../crates/platform-n64/shim/rsp_trino.S\n\
            mips64-elf-size -G $u.elf\n\
            mips64-elf-objcopy -O binary -j .text $u.elf $u.text.bin\n\
            mips64-elf-objcopy -O binary -j .data $u.elf $u.data.bin\n\
            for s in text data; do mips64-elf-objcopy -I binary -O elf32-bigmips -B mips4300 \
              --redefine-sym _binary_${{u}}_${{s}}_bin_start=${{u}}_${{s}}_start \
              --redefine-sym _binary_${{u}}_${{s}}_bin_end=${{u}}_${{s}}_end \
              --redefine-sym _binary_${{u}}_${{s}}_bin_size=${{u}}_${{s}}_size \
              --set-section-alignment .data=8 --rename-section .text=.data \
              $u.${{s}}.bin $u.${{s}}.o || return 1; done; }}\n\
            mkdir -p target/n64/rsp\n\
            run mips64-elf-gcc -c -std=gnu17 -march=vr4300 -mtune=vr4300 -mabi=o64 -O2 -G0 \
            -ffunction-sections -fdata-sections -DN64 \
            -I$N64_INST/mips64-elf/include \
            crates/platform-n64/shim/trino_shim.c -o target/n64/shim.o\n\
            run mips64-elf-gcc -c -std=gnu17 -march=vr4300 -mtune=vr4300 -mabi=o64 -O2 -G0 \
            -ffunction-sections -fdata-sections -DN64 \
            -I$N64_INST/mips64-elf/include \
            crates/platform-n64/shim/trino_opus.c -o target/n64/opus.o\n\
            cd target/n64/rsp\n\
            for u in rsp_trino rsp_trino_color rsp_trino_tex rsp_trino_water; do run overlay $u; done\n\
            drain\n\
            mips64-elf-ld -relocatable rsp_trino.text.o rsp_trino.data.o \
              rsp_trino_color.text.o rsp_trino_color.data.o \
              rsp_trino_tex.text.o rsp_trino_tex.data.o \
              rsp_trino_water.text.o rsp_trino_water.data.o -o ../rsp_trino.o",
            jobs = crate::threads::build(),
        );
        let status = docker_sh(p, false, &script).map_err(|e| format!("docker: {e}"))?;
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
    // Non-boot groups: their lines go to `groups/<g>.tbnd` (issue #56).
    let mut groups = crate::console_groups::GroupIndex::new(Platform::N64);

    for (name, decl) in &manifest.sprites {
        let logical = format!("sprites/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        // A declared format, or `auto`'s pick among the formats whose
        // result is known (and `min_psnr`, when set).
        let (w, h, mut rgba) = trino_asset_pipeline::decode_png(&source)?;
        // A `grading` LUT is baked into the pixels (issue #64).
        let graded = match manifest.console_grading(
            &root.join("assets"),
            Platform::N64,
            decl.grading.as_deref(),
        )? {
            Some(blob) => {
                let lut = trino_core::Lut::parse(&blob).map_err(|e| format!("{logical}: {e:?}"))?;
                trino_asset_pipeline::lut::grade_rgba(&mut rgba, &lut);
                true
            }
            None => false,
        };
        let format = trino_asset_pipeline::texture::choose(
            Platform::N64,
            &logical,
            trino_asset_pipeline::texture::Source::Sprite,
            &decl.settings(Platform::N64),
            w,
            h,
            &rgba,
        )?
        .format;
        // Manifest names follow the hardware format; mksprite has its own
        // vocabulary (RGBA5551 is 16-bit color = RGBA16 there).
        let mksprite_format = match format.as_str() {
            "RGBA5551" => "RGBA16",
            other => other, // CI4 / CI8 match 1:1
        };
        // A mask in I4/I8 is staged with its coverage as the intensity
        // (`texture::n64::stage`); anything else goes as the master file.
        let staged_png = stage.join(format!("{id:08x}.png"));
        match trino_asset_pipeline::N64Format::parse(&format).map(|f| {
            trino_asset_pipeline::texture::n64::stage(
                trino_asset_pipeline::texture::Source::Sprite,
                f,
                &rgba,
            )
        }) {
            Some(std::borrow::Cow::Owned(staged)) => {
                let png = trino_asset_pipeline::font::encode_png(w, h, &staged)?;
                std::fs::write(&staged_png, png).map_err(|e| e.to_string())?;
            }
            _ if graded => {
                let png = trino_asset_pipeline::font::encode_png(w, h, &rgba)?;
                std::fs::write(&staged_png, png).map_err(|e| e.to_string())?;
            }
            _ => {
                std::fs::copy(&source, &staged_png).map_err(|e| e.to_string())?;
            }
        }
        let compress = crate::console_groups::mksprite_compress(
            crate::console_groups::n64_sprite_level(&manifest, &logical)?,
        );
        script.push_str(&format!(
            "mksprite {compress}--format {mksprite_format} -o target/n64/filesystem target/n64/stage/{id:08x}.png\n"
        ));
        groups.route(
            &manifest,
            &logical,
            &format!("{id:08x}\tsprite\t{id:08x}.sprite\n"),
            &mut index,
        )?;
    }
    // Sounds and music: wav64 files the mixer streams from the ROM, with
    // the codec and bus as index columns 5 and 6 (issue #53) after an empty
    // tier column (#54: every tier loads them).
    let mut opus = false;
    let audio_sections = [
        ("sounds", "sound", false, &manifest.sounds),
        ("music", "music", true, &manifest.music),
    ];
    for (prefix, kind, music, section) in audio_sections {
        for (name, decl) in section {
            let logical = format!("{prefix}/{name}");
            let id = trino_core::asset_id(&logical);
            let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
            let settings = decl
                .audio_settings(music, Platform::N64)
                .map_err(|e| format!("`{logical}`: {e}"))?;
            let file = format!("{id:08x}.wav64");
            let staged = stage.join(format!("{id:08x}.wav"));
            if stage_audio(&source, &settings, &staged, &fsdir.join(&file))
                .map_err(|e| format!("`{logical}`: {e}"))?
            {
                // Opus: audioconv64 encodes it (CELT at 48 kHz).
                script.push_str(&format!(
                    "audioconv64 --wav-compress 3 --wav-loop {} -o target/n64/filesystem \
                     target/n64/stage/{id:08x}.wav\n",
                    settings.looped
                ));
                opus = true;
            }
            // An empty tier column (every tier loads it, #54), then the
            // codec and the bus.
            let line = format!(
                "{id:08x}\t{kind}\t{file}\t\t{}\t{}\n",
                settings.codec.label(),
                settings.bus.label()
            );
            groups.route(&manifest, &logical, &line, &mut index)?;
        }
    }
    // The ROM links the Opus decoder only when an asset needs it.
    let opus_marker = root.join(OPUS_MARKER);
    if opus {
        std::fs::write(&opus_marker, "1").map_err(|e| e.to_string())?;
    } else {
        let _ = std::fs::remove_file(&opus_marker);
    }
    // The project's mix (`assets/audio.toml`) as the `audio/mixer` asset the
    // runtime applies at boot.
    let mix_path = root
        .join("assets")
        .join(trino_asset_pipeline::audio::MixFile::FILE);
    if mix_path.exists() {
        let text = std::fs::read_to_string(&mix_path)
            .map_err(|e| format!("{}: {e}", mix_path.display()))?;
        let blob = trino_asset_pipeline::audio::MixFile::parse(&text)
            .and_then(|m| m.bake())
            .map_err(|e| format!("{}: {e}", mix_path.display()))?;
        let id = trino_core::asset_id(trino_core::MixerConfig::LOGICAL);
        std::fs::write(fsdir.join(format!("{id:08x}.mixer")), blob).map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tmixer\t{id:08x}.mixer\n"));
    }
    // The project's UI screens and string tables (issue #58): `ui/<name>`
    // and `i18n/<lang>` blobs, resident from boot.
    for b in trino_asset_pipeline::ui::bake(root, &manifest, Platform::N64)? {
        let id = trino_core::asset_id(&b.logical);
        std::fs::write(fsdir.join(format!("{id:08x}.blob")), &b.bytes)
            .map_err(|e| e.to_string())?;
        index.push_str(&format!("{id:08x}\tblob\t{id:08x}.blob\n"));
    }
    for (name, decl) in &manifest.models {
        // A model only the expanded tier names is in the image too, loaded
        // on that hardware only (issue #54).
        let Some(tier) = trino_asset_pipeline::tier::model_tier(decl, Platform::N64) else {
            continue;
        };
        let logical = format!("models/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        // TMDL is portable: same blob on every platform, no container tool;
        // a `grading` LUT is baked into its colors here (issue #64).
        let mut opts = decl.options();
        opts.grading = manifest.console_grading(
            &root.join("assets"),
            Platform::N64,
            decl.grading.as_deref(),
        )?;
        let blob = trino_asset_pipeline::bake_model(&source, &opts)?;
        let file = format!("{id:08x}.tmdl");
        std::fs::write(fsdir.join(&file), blob).map_err(|e| e.to_string())?;
        let line = trino_asset_pipeline::tier::index_line(id, "model", &file, tier);
        groups.route(&manifest, &logical, &line, &mut index)?;
    }

    for (name, decl) in &manifest.fonts {
        let logical = format!("fonts/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        let font = trino_asset_pipeline::bake_font(&source, decl)
            .map_err(|e| format!("font `{logical}`: {e}"))?;
        let format = trino_asset_pipeline::texture::choose(
            Platform::N64,
            &logical,
            trino_asset_pipeline::texture::Source::FontAtlas,
            &decl.settings(Platform::N64),
            font.width,
            font.height,
            &font.rgba,
        )?
        .format;
        let mksprite_format = match format.as_str() {
            "RGBA5551" => "RGBA16",
            other => other,
        };
        // Atlas -> sprite (mksprite, same handle); table -> verbatim TFNT.
        // Intensity formats take alpha from the color: stage the coverage
        // as the intensity, so antialiased edges stay soft.
        let rgba = match trino_asset_pipeline::N64Format::parse(&format) {
            Some(f) => trino_asset_pipeline::texture::n64::stage(
                trino_asset_pipeline::texture::Source::FontAtlas,
                f,
                &font.rgba,
            ),
            None => std::borrow::Cow::Borrowed(&font.rgba[..]),
        };
        let png = trino_asset_pipeline::font::encode_png(font.width, font.height, &rgba)?;
        std::fs::write(stage.join(format!("{id:08x}.png")), png).map_err(|e| e.to_string())?;
        let compress = crate::console_groups::mksprite_compress(
            crate::console_groups::n64_sprite_level(&manifest, &logical)?,
        );
        script.push_str(&format!(
            "mksprite {compress}--format {mksprite_format} -o target/n64/filesystem target/n64/stage/{id:08x}.png\n"
        ));
        std::fs::write(fsdir.join(format!("{id:08x}.font")), &font.table)
            .map_err(|e| e.to_string())?;
        let lines = format!("{id:08x}\tsprite\t{id:08x}.sprite\n{id:08x}\tfont\t{id:08x}.font\n");
        groups.route(&manifest, &logical, &lines, &mut index)?;
    }
    for (name, decl) in &manifest.blobs {
        let logical = format!("blobs/{name}");
        let id = trino_core::asset_id(&logical);
        let source = resolve_source(&root.join("assets"), Platform::N64, &decl.file)?;
        std::fs::copy(&source, fsdir.join(format!("{id:08x}.blob"))).map_err(|e| e.to_string())?;
        groups.route(
            &manifest,
            &logical,
            &format!("{id:08x}\tblob\t{id:08x}.blob\n"),
            &mut index,
        )?;
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
        // Each hardware tier's cap: a `base` and an `expanded` copy when
        // they differ (issue #54).
        let twins = trino_asset_pipeline::tier_twins(Platform::N64, |t| {
            trino_asset_pipeline::particles::bake(&source, t)
        })?;
        for twin in twins {
            std::fs::write(fsdir.join(twin.file(id, "blob")), &twin.bytes)
                .map_err(|e| e.to_string())?;
            groups.route(
                &manifest,
                &logical,
                &twin.index_line(id, "blob", "blob"),
                &mut index,
            )?;
        }
    }
    for (name, decl) in &manifest.scenes {
        let logical = format!("scenes/{name}");
        let id = trino_core::asset_id(&logical);
        // TSCN is portable; the variants resolve per hardware tier (a
        // `base` and an `expanded` copy when they differ, issue #54).
        let twins = trino_asset_pipeline::tier_twins(Platform::N64, |t| {
            trino_asset_pipeline::model::bake_console_scene(
                &root.join("assets"),
                &manifest,
                decl,
                Platform::N64,
                t,
            )
        })?;
        for twin in twins {
            std::fs::write(fsdir.join(twin.file(id, "blob")), &twin.bytes)
                .map_err(|e| e.to_string())?;
            groups.route(
                &manifest,
                &logical,
                &twin.index_line(id, "blob", "blob"),
                &mut index,
            )?;
        }
    }

    index.push_str(&groups.index_lines());
    index.insert_str(0, &groups.n64_compression_lines(&manifest)?);
    std::fs::write(fsdir.join("index.tsv"), index).map_err(|e| e.to_string())?;
    if test_mode {
        std::fs::write(fsdir.join("test_mode"), "1").map_err(|e| e.to_string())?;
    }
    // Compress in place what the groups ask for (after mksprite made the
    // sprites); the DFS's `groups/` exists before the container writes in
    // it, so the bundles below can be written by this user.
    script.push_str(&groups.n64_compress_script(&manifest, "target/n64/filesystem")?);
    std::fs::create_dir_all(fsdir.join("groups")).map_err(|e| e.to_string())?;

    println!("[n64] baking assets in container...");
    let status = docker_sh(p, true, &script).map_err(|e| format!("docker: {e}"))?;
    if !status.success() {
        return Err("asset baking failed".into());
    }
    for b in groups.write_bundles(&manifest, &fsdir)? {
        println!("{}", b.line("n64"));
    }
    Ok(())
}

/// `target/n64/opus` (in the project): the bake found an Opus asset, so
/// [`pack_rom`] links the shim's `trino_opus.o` (and with it libopus).
const OPUS_MARKER: &str = "target/n64/opus";

/// Bake one sound for the N64 with `trino_asset_pipeline::audio::encode`
/// (the master as the console plays it: mono, at its `sample_rate` or at
/// most 44.1 kHz, the mixer's channel limit, in whole 16-sample blocks),
/// written as a VADPCM wav64 by our encoder (audioconv64's turned
/// near-silent passages of castle64's theme into full-scale clicks, 7.6 dB
/// SNR against our 30.8) or a raw one — the same bytes `audio inspect`,
/// `audio preview` and the editor measure. Opus is staged as a WAV at
/// `staged` for audioconv64 instead: returns true.
fn stage_audio(
    source: &Path,
    settings: &trino_asset_pipeline::audio::AudioSettings,
    staged: &Path,
    out: &Path,
) -> Result<bool, String> {
    use trino_asset_pipeline::audio::{self, Codec, N64_MAX_RATE};
    let master = trino_asset_pipeline::decode_master(source)?;
    let err = |e: String| format!("{}: {e}", source.display());
    if settings.codec == Codec::Opus {
        // audioconv64 encodes Opus from a WAV (and resamples it to 48 kHz).
        let rate = settings
            .sample_rate
            .unwrap_or(master.rate.min(N64_MAX_RATE));
        let samples = audio::resample(&master.samples, master.rate, rate);
        let pcm = audio::n64_pcm(audio::to_pcm16(&samples), settings.looped).map_err(err)?;
        write_wav_mono(staged, rate, &pcm)?;
        return Ok(true);
    }
    let enc = audio::encode(Platform::N64, settings, &master, 0).map_err(err)?;
    let bytes = enc.blob.ok_or_else(|| err("no wav64".into()))?;
    std::fs::write(out, bytes).map_err(|e| format!("{}: {e}", out.display()))?;
    Ok(false)
}

fn write_wav_mono(out: &Path, rate: u32, pcm: &[i16]) -> Result<(), String> {
    let spec = hound::WavSpec {
        channels: 1,
        sample_rate: rate,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut w = hound::WavWriter::create(out, spec).map_err(|e| e.to_string())?;
    for &s in pcm {
        w.write_sample(s).map_err(|e| e.to_string())?;
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
        .args(p.trace_features())
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
    // After the Rust code, so linking it moves none of that.
    let opus_obj = if p.root.join(OPUS_MARKER).exists() {
        "/engine/target/n64/opus.o"
    } else {
        ""
    };
    let script = format!(
        "set -e\n\
         {pad_step}\
         mips64-elf-g++ -o target/n64/{name}.elf /engine/target/n64/shim.o /engine/target/n64/rsp_trino.o {pad_obj} \
         -Wl,-u,memcpy -Wl,-u,memset -Wl,-u,memmove -Wl,-u,memcmp -Wl,-lc \
         {staticlib} {opus_obj} -lc -mabi=o64 -Wl,-g -Wl,-L$N64_INST/mips64-elf/lib \
         -Wl,-ldragon -Wl,-lm -Wl,-ldragonsys -Wl,-Tn64.ld \
         -Wl,--gc-sections -Wl,--no-warn-mismatch \
         -Wl,--wrap -Wl,__do_global_ctors \
         -Wl,-Map=target/n64/{name}.map\n\
         mips64-elf-size -G target/n64/{name}.elf\n\
         mips64-elf-nm -S -C target/n64/{name}.elf > target/n64/{name}.syms\n\
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
    let syms_path = p.root.join(format!("target/n64/{name}.syms"));
    let syms =
        std::fs::read_to_string(&syms_path).map_err(|e| format!("{}: {e}", syms_path.display()))?;
    println!("[n64] {}", check_hot_symbols(&syms, p.trace)?);
    println!("[n64] ROM: {}", p.rom().display());
    Ok(())
}

/// The N64's direct-mapped I-cache is 16 KB: the unlit, unskinned T&L body
/// (`tessellate_joints::<false, false>`, the one most CPU draws run) must
/// fit in it (AGENTS.md pitfall: 15.6 → 18.0 KB made every unlit draw
/// 10–35 % slower).
pub const TNL_ICACHE_BYTES: u64 = 16 * 1024;

/// The ROM's symbols (`mips64-elf-nm -S -C`): the hot T&L body within the
/// I-cache, and — unless the build is `--trace` — none of the trace
/// recorder (`trino_core::trace::global`, the zones, a host's ring): zones
/// compile out completely (issue #62).
pub fn check_hot_symbols(syms: &str, trace: bool) -> Result<String, String> {
    let size_of = |name: &str| {
        syms.lines().find_map(|l| {
            let mut f = l.splitn(4, ' ');
            let (_, size, _, sym) = (f.next()?, f.next()?, f.next()?, f.next()?);
            (sym == name).then(|| u64::from_str_radix(size, 16).ok())?
        })
    };
    let tnl = "trino_core::render3d::tessellate_joints::<false, false>";
    let size = size_of(tnl).ok_or(format!("{tnl}: not in the ROM's symbols"))?;
    if size > TNL_ICACHE_BYTES {
        return Err(format!(
            "{tnl} is {size} bytes, past the N64's {TNL_ICACHE_BYTES}-byte I-cache \
             (keep new per-vertex work out of it: AGENTS.md pitfalls)"
        ));
    }
    let recorder: Vec<&str> = syms
        .lines()
        .filter(|l| {
            l.contains("trino_core::trace::global")
                || l.contains("trino_core::trace::Zone")
                || l.contains("trace_start::RING")
        })
        .collect();
    if !trace && !recorder.is_empty() {
        return Err(format!(
            "the trace recorder is linked into a build without --trace (zones must compile \
             out): {}",
            recorder.join("; ")
        ));
    }
    if trace && recorder.is_empty() {
        return Err("a --trace build without the trace recorder (trino-core/trace off?)".into());
    }
    Ok(format!(
        "hot code: {tnl} {size} bytes (I-cache {TNL_ICACHE_BYTES}); trace recorder {}",
        if trace { "linked (--trace)" } else { "absent" }
    ))
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

/// ares's Expansion Pak setting (`--expansion-pak on|off`, issue #54).
fn pak_setting(pak: bool) -> &'static str {
    if pak {
        "Nintendo64/ExpansionPak=true"
    } else {
        "Nintendo64/ExpansionPak=false"
    }
}

pub fn run(p: &Project, hw: &crate::hwtier::Options) -> ExitCode {
    let code = build(p, false);
    if code != ExitCode::SUCCESS {
        return code;
    }
    let Some(ares) = ares_exe(p) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };
    let settings: &[&str] = match hw.pak_run() {
        Some(pak) => {
            println!(
                "[n64] launching ares ({} the Expansion Pak)...",
                if pak { "with" } else { "without" }
            );
            &["--setting", pak_setting(pak)]
        }
        None => {
            println!("[n64] launching ares...");
            &[]
        }
    };
    match Command::new(&ares).args(settings).arg(p.rom()).spawn() {
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
pub fn watch(p: &Project, hw: &crate::hwtier::Options) -> ExitCode {
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

    let settings: &[&str] = match hw.pak_run() {
        Some(pak) => &["--setting", pak_setting(pak)],
        None => &[],
    };
    let launch = |ares: &Path, rom: &Path| -> std::io::Result<std::process::Child> {
        Command::new(ares).args(settings).arg(rom).spawn()
    };
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

/// Build the test ROM and boot it in ares once per Expansion Pak setting
/// `pak` asks for (`--expansion-pak on|off|both`; on by default, ares's own
/// default): one ROM detects its tier at boot (issue #54).
pub fn test(p: &Project, bless: bool, pak: crate::hwtier::Mode) -> ExitCode {
    let code = build(p, true);
    if code != ExitCode::SUCCESS {
        return code;
    }
    for &on in pak.runs() {
        let code = boot_test(p, bless, on);
        if code != ExitCode::SUCCESS {
            return code;
        }
    }
    ExitCode::SUCCESS
}

/// Boot the test ROM in ares (with or without the Expansion Pak) and watch
/// stdout for the ISViewer magic strings and the `TRINO_HW` line. Kills the
/// emulator on verdict or timeout.
fn boot_test(p: &Project, bless: bool, pak: bool) -> ExitCode {
    let root = &p.root;
    let Some(ares) = ares_exe(p) else {
        eprintln!("[n64] ares not found (expected ares-v148/ares.exe at the repo root)");
        return ExitCode::FAILURE;
    };

    let timeout = crate::emulators::timeout_secs();
    let idle = crate::emulators::idle_secs();
    println!(
        "[n64] booting test ROM in ares {} the Expansion Pak ({timeout}s timeout, {idle}s idle limit)...",
        if pak { "with" } else { "without" }
    );
    // Native-resolution RDP: the golden frame is compared pixel-exactly.
    let (mut cmd, container) = ares_command(
        &ares,
        &p.rom(),
        &[
            "--no-file-prompt",
            "--setting",
            "Video/Quality=SD",
            "--setting",
            pak_setting(pak),
        ],
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
    let mut traces = crate::trace::Capture::default();
    let mut frames: Vec<crate::fbgolden::FbCapture> = Vec::new();
    let mut perf_scenes = Vec::new();
    let mut micro_scenes = Vec::new();
    let mut frame_lines = Vec::new();
    let mut hw_line = None;
    let mut tier_step = None;
    let mut quality_step = None;
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
                // Trace lines are collected, not echoed (issue #62).
                if traces.feed(&line) {
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
                if hw_line.is_none() && line.contains("TRINO_HW n64") {
                    hw_line = Some(line.clone());
                }
                if line.contains("TRINO_STEP:hw tier ") {
                    tier_step = Some(line.clone());
                }
                if line.contains("TRINO_STEP:quality ") {
                    quality_step = Some(line.clone());
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

    // The boot's tier against ares's setting (issue #54).
    let verdict = verdict.and_then(|()| {
        let hw = crate::hwtier::check_n64(hw_line.as_deref(), pak)?;
        println!("[n64] hardware tier: {hw}");
        // The engine self-test loaded that tier's data (castle64's step).
        if p.is_engine {
            let data = crate::hwtier::check_data(hw_line.as_deref(), tier_step.as_deref())?;
            println!("[n64] tier data: {data}");
            // A console's quality preset is fixed: Low with or without the
            // Pak (issue #55).
            let q = crate::hwtier::check_quality(quality_step.as_deref(), "low")?;
            println!("[n64] quality: {q}");
        }
        Ok(())
    });
    // The run's trace (the self-test's `trace` step; with --trace the
    // engine's zones too) into target/traces/ (issue #62).
    let verdict = verdict.and_then(|()| {
        let name = format!("n64{}", if p.trace { "-trace" } else { "" });
        let msg = traces.finish(&p.engine, &name, p.is_engine)?;
        println!("[n64] trace: {msg}");
        Ok(())
    });
    let verdict = if p.is_engine && p.trace {
        // An instrumented ROM is not the gated one: report, never gate.
        verdict.map(|()| {
            println!("[n64] --trace build: perf gates and goldens skipped (instrumented code)");
            if let Ok(t) = crate::perf::report_only(&perf_scenes) {
                println!("[n64] perf with the zones in (emulated us per frame):\n{t}");
            }
        })
    } else if p.is_engine {
        verdict.and_then(|()| engine_gates(root, &frames, &perf_scenes, &micro_scenes, bless))
    } else {
        verdict.and_then(|()| {
            project_gates(p, "n64", &p.test.n64_goldens, &frames, bless, None)?;
            frame_gate(&frame_lines, p.test.n64_max_frame_us)
        })
    };

    match verdict {
        Ok(()) => {
            println!(
                "[n64] TEST PASS ({} the Expansion Pak)",
                if pak { "with" } else { "without" }
            );
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
    // (fogged: the CPU T&L path), castle64's hub (the RSP T&L path), the
    // showcase's river (water, ADR 0015: the `showcase-river` perf scene)
    // and blobs on a large floor (shadows, ADR 0025: no z-fighting out to
    // 100 m).
    for (n, name) in ["showcase", "castle64_hub", "showcase_river", "shadows_far"]
        .iter()
        .enumerate()
    {
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
    fn hot_symbols_fit_the_icache_and_zones_compile_out() {
        let tnl = "800fda88 00003f58 t trino_core::render3d::tessellate_joints::<false, false>\n\
                   80105d20 00004098 t trino_core::render3d::tessellate_joints::<true, false>\n";
        let ring = "801ad608 0000c41c D trino_platform_n64::runtime::trace_start::RING\n";
        let global = "80050000 00000040 t trino_core::trace::global::begin\n";
        let ok = check_hot_symbols(tnl, false).unwrap();
        assert!(ok.contains("16216 bytes"), "{ok}");
        assert!(check_hot_symbols(&format!("{tnl}{ring}"), false).is_err());
        assert!(check_hot_symbols(&format!("{tnl}{global}"), false).is_err());
        assert!(check_hot_symbols(&format!("{tnl}{ring}{global}"), true).is_ok());
        assert!(
            check_hot_symbols(tnl, true).is_err(),
            "--trace must link the recorder"
        );
        let big = tnl.replace("00003f58", "00004100");
        assert!(
            check_hot_symbols(&big, false)
                .unwrap_err()
                .contains("I-cache")
        );
        assert!(check_hot_symbols("", false).is_err());
    }

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
            w.write_sample((i * 100) as i16).unwrap();
        }
        w.finalize().unwrap();
    }

    fn settings(codec: &str, looped: bool, rate: Option<u32>) -> AudioSettings {
        let decl: trino_asset_pipeline::manifest::SoundDecl = toml::from_str(&format!(
            "file = \"x.wav\"\ncodec = \"{codec}\"\nloop = {looped}\n{}",
            rate.map_or(String::new(), |r| format!("sample_rate = {r}\n"))
        ))
        .unwrap();
        decl.audio_settings(false, Platform::N64).unwrap()
    }

    use trino_asset_pipeline::audio::AudioSettings;
    use trino_asset_pipeline::audio::vadpcm::{Wav64Format, decode_wav64, parse_wav64};

    #[test]
    fn audio_bakes_to_wav64_per_codec_with_whole_block_loops() {
        let dir = tempfile::tempdir().unwrap();
        let (src, staged, out) = (
            dir.path().join("in.wav"),
            dir.path().join("staged.wav"),
            dir.path().join("out.wav64"),
        );
        write_wav(&src, 2, 37);
        // A looped VADPCM sound: mono, 37 frames trimmed to 32, ours.
        assert!(!stage_audio(&src, &settings("vadpcm", true, None), &staged, &out).unwrap());
        let (info, pcm) = decode_wav64(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(info.format, Wav64Format::Vadpcm);
        assert_eq!((info.channels, info.sample_rate), (1, 22_050));
        assert_eq!(
            (info.frames, info.loop_len),
            (32, 32),
            "37 frames trim to 32"
        );
        assert_eq!(pcm.len(), 32);
        // A raw one-shot is padded to 48 frames of which the last 11 are
        // silence (stereo averaged).
        assert!(!stage_audio(&src, &settings("pcm", false, None), &staged, &out).unwrap());
        let (info, pcm) = decode_wav64(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(
            (info.format, info.frames, info.loop_len),
            (Wav64Format::Raw, 48, 0)
        );
        assert_eq!(pcm[1], 250, "frame 1 = (200 + 300) / 2");
        assert!(pcm[37..].iter().all(|&s| s == 0));
        // A resampled one.
        stage_audio(&src, &settings("pcm", false, Some(11_025)), &staged, &out).unwrap();
        let info = parse_wav64(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!(
            (info.sample_rate, info.frames),
            (11_025, 32),
            "19 frames, padded"
        );
        // Opus goes to audioconv64 as a mono WAV.
        assert!(stage_audio(&src, &settings("opus", false, None), &staged, &out).unwrap());
        let r = hound::WavReader::open(&staged).unwrap();
        assert_eq!((r.spec().channels, r.len()), (1, 48));
        // A loop under 16 frames repeats whole: 15 frames → 240.
        write_wav(&src, 1, 15);
        stage_audio(&src, &settings("vadpcm", true, None), &staged, &out).unwrap();
        let info = parse_wav64(&std::fs::read(&out).unwrap()).unwrap();
        assert_eq!((info.frames, info.loop_len), (240, 240));
        write_wav(&src, 1, 0);
        let err = stage_audio(&src, &settings("vadpcm", true, None), &staged, &out);
        assert!(err.unwrap_err().contains("can't loop"));
        assert!(
            stage_audio(
                &dir.path().join("missing.wav"),
                &settings("vadpcm", false, None),
                &staged,
                &out
            )
            .is_err()
        );
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
