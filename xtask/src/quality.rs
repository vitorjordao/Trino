//! `cargo xtask quality show [--json]` — the project's quality presets
//! (issue #55): `trino.toml`'s `[pc.quality]` (the first launch's default,
//! the presets file) and what Low, Medium, High and Ultra mean once
//! `assets/quality.ron` changes the built-in values. The editor's Quality
//! tab shows and edits the same file.
//!
//! `cargo xtask quality set <low|medium|high|ultra> <key>=<value>.. [--json]`
//! — change a preset's values in `assets/quality.ron` (created on the first
//! change; the file keeps only what differs from the built-in values).
//! `cargo xtask quality reset [<preset>..]` drops a preset's changes.
//!
//! `cargo xtask quality check` — validate both, as the bake does.
//!
//! `cargo xtask quality bench [--json]` — the first launch's calibration on
//! this machine: the project's PC host opens its window, draws the
//! built-in bench scene at each preset (Low first, at most ~2 s), prints
//! the medians and its pick, and exits (`TRINO_QUALITY_BENCH=1`).

use std::path::Path;
use std::process::{Command, ExitCode, Stdio};

use trino_asset_pipeline::quality::{ProjectQuality, QualityFile, frame_limit_key};
use trino_core::{PresetTable, QualityPreset, QualitySettings};

use crate::project::Project;

const USAGE: &str = "xtask: usage:
  cargo xtask quality show [--json]
  cargo xtask quality set <low|medium|high|ultra> <key>=<value>.. [--json]
  cargo xtask quality reset [<preset>..]
  cargo xtask quality check
  cargo xtask quality bench [--json]";

pub fn run(project: &Project, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let args: Vec<&str> = args.iter().copied().filter(|a| *a != "--json").collect();
    let root = &project.root;
    let result = match args.as_slice() {
        ["show"] => show(root, json),
        ["check"] => check(root),
        ["set", preset, changes @ ..] if !changes.is_empty() => set(root, preset, changes, !json)
            .and_then(|()| if json { show(root, true) } else { Ok(()) }),
        ["reset", presets @ ..] => reset(root, presets),
        ["bench"] => bench(project, json),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("{e}");
            ExitCode::FAILURE
        }
    }
}

fn load(root: &Path) -> Result<(ProjectQuality, PresetTable), String> {
    let q = ProjectQuality::load(root)?;
    let table = q.table(root)?;
    Ok((q, table))
}

/// One preset as JSON (every field).
pub fn preset_json(q: &QualitySettings) -> serde_json::Value {
    // Hundredths, as the file writes them (an f32 widened shows its noise).
    let n = |v: f32| (v as f64 * 100.0).round() / 100.0;
    serde_json::json!({
        "preset": q.preset.key(),
        "render_scale": n(q.render_scale),
        "dynamic_resolution": q.dynamic_resolution.map(|d| serde_json::json!({
            "target_us": d.target_us,
            "min_scale": n(d.min_scale),
            "max_scale": n(d.max_scale),
        })),
        "msaa": q.msaa,
        "ao": q.ao.key(),
        "shadows": q.shadows.key(),
        "post": q.post.key(),
        "lod_bias": n(q.lod_bias),
        "particle_scale": n(q.particle_scale),
        "texture_filter": trino_core::quality::filter_key(q.texture_filter),
        "anisotropy": q.anisotropy,
        "sharpen": n(q.sharpen),
        "frame_limit": frame_limit_key(q.frame_limit),
    })
}

fn show(root: &Path, json: bool) -> Result<(), String> {
    let (q, table) = load(root)?;
    let file = root.join(&q.presets);
    if json {
        let doc = serde_json::json!({
            "default": q.default,
            "presets_file": q.presets,
            "file_exists": file.exists(),
            "presets": table.presets.iter().map(preset_json).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
        return Ok(());
    }
    println!(
        "default {}  presets {}{}",
        q.default,
        q.presets.display(),
        if file.exists() {
            ""
        } else {
            " (none: built-in)"
        }
    );
    println!(
        "  {:<7} {:>5} {:<13} {:>4} {:<6} {:<7} {:<7} {:>4} {:>5} {:>5} {:>5} {:<8}",
        "preset",
        "scale",
        "dynres",
        "msaa",
        "ao",
        "shadows",
        "post",
        "lod",
        "parts",
        "aniso",
        "sharp",
        "limit"
    );
    let built_in = PresetTable::BUILT_IN;
    for (p, b) in table.presets.iter().zip(&built_in.presets) {
        let dynres = p.dynamic_resolution.map_or("off".to_string(), |d| {
            format!("{:.2}-{:.2}", d.min_scale, d.max_scale)
        });
        println!(
            "  {:<7} {:>5.2} {:<13} {:>4} {:<6} {:<7} {:<7} {:>4.2} {:>5.2} {:>5} {:>5.2} {:<8}{}",
            p.preset.key(),
            p.render_scale,
            dynres,
            p.msaa,
            p.ao.key(),
            p.shadows.key(),
            p.post.key(),
            p.lod_bias,
            p.particle_scale,
            p.anisotropy,
            p.sharpen,
            frame_limit_key(p.frame_limit),
            if p != b { "  (changed)" } else { "" }
        );
    }
    Ok(())
}

fn check(root: &Path) -> Result<(), String> {
    let (q, _) = load(root)?;
    let file = root.join(&q.presets);
    println!(
        "quality ok: default {}, {}",
        q.default,
        if file.exists() {
            file.display().to_string()
        } else {
            "built-in presets".to_string()
        }
    );
    Ok(())
}

fn preset_arg(s: &str) -> Result<QualityPreset, String> {
    QualityPreset::parse(s)
        .filter(|p| *p != QualityPreset::Custom)
        .ok_or_else(|| format!("xtask: `{s}` is not a preset (low, medium, high, ultra)"))
}

/// Write `table` as the project's file (validated by reading it back);
/// `say` prints where (not under `--json`, whose stdout is the document).
fn write(root: &Path, q: &ProjectQuality, table: &PresetTable, say: bool) -> Result<(), String> {
    let text = QualityFile::from_table(table).to_ron();
    let back = QualityFile::parse(&text)
        .and_then(|f| f.table())
        .map_err(|e| format!("xtask: {e}"))?;
    if back != *table {
        return Err("xtask: a value is out of its range (see `quality show`)".into());
    }
    let path = root.join(&q.presets);
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    }
    std::fs::write(&path, text).map_err(|e| format!("{}: {e}", path.display()))?;
    if say {
        println!("wrote {}", path.display());
    }
    Ok(())
}

fn set(root: &Path, preset: &str, changes: &[&str], say: bool) -> Result<(), String> {
    let p = preset_arg(preset)?;
    let (q, mut table) = load(root)?;
    let i = p.index().expect("a named preset");
    let mut s = table.presets[i];
    for c in changes {
        let Some((k, v)) = c.split_once('=') else {
            return Err(format!("xtask: `{c}` is not key=value"));
        };
        if k.trim() == "preset" || !s.set_field(k, v) {
            return Err(format!(
                "xtask: `{c}`: keys are render_scale, dynamic_resolution (off|on|<target_us>,<min>,<max>), \
                 msaa, ao, shadows, post, lod_bias, particle_scale, texture_filter, anisotropy, \
                 sharpen, frame_limit (vsync|unlimited|<fps>)"
            ));
        }
    }
    // Strict ranges (the file's), not the runtime's clamping.
    let decl = trino_asset_pipeline::quality::PresetDecl::full(&s);
    decl.apply(table.presets[i])
        .map_err(|e| format!("xtask: {}: {e}", p.key()))?;
    table.presets[i] = s;
    write(root, &q, &table, say)
}

fn reset(root: &Path, presets: &[&str]) -> Result<(), String> {
    let (q, mut table) = load(root)?;
    let which: Vec<QualityPreset> = if presets.is_empty() {
        QualityPreset::NAMED.to_vec()
    } else {
        presets
            .iter()
            .map(|p| preset_arg(p))
            .collect::<Result<_, _>>()?
    };
    for p in which {
        let i = p.index().expect("a named preset");
        table.presets[i] = PresetTable::BUILT_IN.presets[i];
    }
    write(root, &q, &table, true)
}

/// The bench line the PC host prints (`TRINO_QUALITY: bench ...`), parsed.
#[derive(Debug, PartialEq)]
pub struct BenchLine {
    pub results: Vec<(String, u32)>,
    pub pick: String,
    pub gpu: String,
    pub target_us: u32,
    pub elapsed_ms: u32,
    pub picture: String,
    pub adapter: String,
}

pub fn parse_bench_line(line: &str) -> Option<BenchLine> {
    let rest = line.split_once("TRINO_QUALITY: bench ")?.1;
    let (fields, adapter) = rest.split_once(" adapter=")?;
    let mut b = BenchLine {
        results: Vec::new(),
        pick: String::new(),
        gpu: String::new(),
        target_us: 0,
        elapsed_ms: 0,
        picture: String::new(),
        adapter: adapter.trim().to_string(),
    };
    for tok in fields.split_whitespace() {
        let (k, v) = tok.split_once('=')?;
        match k {
            "pick" => b.pick = v.into(),
            "gpu" => b.gpu = v.into(),
            "target_us" => b.target_us = v.parse().ok()?,
            "elapsed_ms" => b.elapsed_ms = v.parse().ok()?,
            "picture" => b.picture = v.into(),
            p if QualityPreset::parse(p).is_some() => b.results.push((p.into(), v.parse().ok()?)),
            _ => {}
        }
    }
    (!b.pick.is_empty()).then_some(b)
}

fn bench(project: &Project, json: bool) -> Result<(), String> {
    let manifest = project.root.join("Cargo.toml");
    let manifest = manifest.to_string_lossy().to_string();
    let mut cmd = Command::new(crate::cargo_bin());
    cmd.arg("run");
    if !project.is_engine {
        cmd.args(["--manifest-path", &manifest]);
    }
    cmd.args(["-p", &project.pc_package])
        .env("TRINO_SIM", "native")
        .env("TRINO_QUALITY_BENCH", "1")
        .env("TRINO_WINDOWED", "1")
        .stdout(Stdio::piped());
    let out = cmd
        .output()
        .map_err(|e| format!("xtask: failed to spawn cargo: {e}"))?;
    let stdout = String::from_utf8_lossy(&out.stdout);
    let Some(b) = stdout.lines().find_map(parse_bench_line) else {
        return Err(format!(
            "xtask: the PC host printed no bench line (exit {:?}):\n{stdout}",
            out.status.code()
        ));
    };
    if json {
        let doc = serde_json::json!({
            "adapter": b.adapter,
            "gpu": b.gpu,
            "picture": b.picture,
            "target_us": b.target_us,
            "elapsed_ms": b.elapsed_ms,
            "pick": b.pick,
            "results": b.results.iter().map(|(p, us)| serde_json::json!({
                "preset": p, "median_us": us, "held": *us <= b.target_us,
            })).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&doc).unwrap());
    } else {
        println!("{} ({}), picture {}", b.adapter, b.gpu, b.picture);
        for (p, us) in &b.results {
            let mark = if *us <= b.target_us {
                "holds"
            } else {
                "misses"
            };
            println!(
                "  {p:<7} {:>7.2} ms  {mark} {:.2} ms",
                *us as f64 / 1000.0,
                b.target_us as f64 / 1000.0
            );
        }
        println!("pick {} in {} ms", b.pick, b.elapsed_ms);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bench_lines_parse() {
        let b = parse_bench_line(
            "TRINO_QUALITY: bench low=3100 medium=5200 high=9800 ultra=16100 pick=high gpu=discrete \
             target_us=14994 elapsed_ms=812 picture=1280x720 adapter=NVIDIA GeForce RTX 3060 (Vulkan)",
        )
        .unwrap();
        assert_eq!(b.results.len(), 4);
        assert_eq!(b.results[3], ("ultra".into(), 16_100));
        assert_eq!((b.pick.as_str(), b.gpu.as_str()), ("high", "discrete"));
        assert_eq!((b.target_us, b.elapsed_ms), (14_994, 812));
        assert_eq!(b.picture, "1280x720");
        assert_eq!(b.adapter, "NVIDIA GeForce RTX 3060 (Vulkan)");
        assert_eq!(parse_bench_line("TRINO_QUALITY: preset=low"), None);
    }
}
