//! `cargo xtask model inspect <file> [--fps N] [--once a,b] [--smooth] [--json]` —
//! bake a model master exactly like the asset pipeline does and report what
//! the engine will see: triangles, vertices, joints, clips, bounds and the
//! N64 budget. The deterministic acceptance gate for art coming out of
//! Blender (or any DCC): if `inspect` is happy, every target is.

use std::path::Path;
use std::process::ExitCode;

use trino_asset_pipeline::{ModelOptions, bake_model, clip_names};
use trino_core::Mesh;
use trino_core::perf::FRAME_30FPS_US;

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let ["inspect", file, rest @ ..] = args else {
        eprintln!(
            "xtask: usage: cargo xtask model inspect <file.glb|.gltf|.prim.ron> [--fps N] [--once clip,clip] [--smooth] [--json]"
        );
        return ExitCode::FAILURE;
    };
    let mut opts = ModelOptions::default();
    let mut json = false;
    let mut it = rest.iter();
    while let Some(a) = it.next() {
        match *a {
            "--json" => json = true,
            "--smooth" => opts.smooth = true,
            "--fps" => match it.next().and_then(|v| v.parse().ok()) {
                Some(v) => opts.anim_fps = v,
                None => {
                    eprintln!("xtask: --fps needs a number");
                    return ExitCode::FAILURE;
                }
            },
            "--once" => {
                opts.once = it
                    .next()
                    .map(|v| v.split(',').map(String::from).collect())
                    .unwrap_or_default()
            }
            other => {
                eprintln!("xtask: unknown flag `{other}`");
                return ExitCode::FAILURE;
            }
        }
    }
    let path = if Path::new(file).is_absolute() {
        Path::new(file).to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| root.to_path_buf())
            .join(file)
    };
    match inspect(&path, &opts) {
        Ok(report) => {
            if json {
                println!("{}", report.json());
            } else {
                print!("{}", report.text());
            }
            if report.errors.is_empty() {
                ExitCode::SUCCESS
            } else {
                ExitCode::FAILURE
            }
        }
        Err(e) => {
            if json {
                println!("{{\"ok\":false,\"error\":{}}}", quote(&e));
            } else {
                eprintln!("error: {e}");
            }
            ExitCode::FAILURE
        }
    }
}

pub struct Report {
    pub file: String,
    pub bytes: usize,
    pub verts: usize,
    pub tris: usize,
    pub joints: usize,
    /// Vertices blending two joints (smooth skinning, `--smooth`).
    pub blended: usize,
    /// Vertices per color slot 0..=4 (TMDL `SLOT`, glTF `_SLOT`); `None`
    /// when the model has no slots.
    pub slots: Option<[usize; 5]>,
    pub clips: Vec<(String, f32, usize, bool)>,
    pub size: [f32; 3],
    pub min: [f32; 3],
    pub radius: f32,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

fn quote(s: &str) -> String {
    format!(
        "\"{}\"",
        s.replace('\\', "\\\\")
            .replace('"', "\\\"")
            .replace('\n', "\\n")
    )
}

/// Estimated emulated-N64 microseconds of one draw (T&L + sort + submit).
fn n64_draw_us(vertices: usize, triangles: usize) -> u32 {
    trino_core::perf::N64_COST.mesh_draw_us(vertices as u32, triangles as u32)
}

impl Report {
    pub fn json(&self) -> String {
        let clips: Vec<String> = self
            .clips
            .iter()
            .map(|(n, d, f, l)| {
                format!(
                    "{{\"name\":{},\"seconds\":{d},\"frames\":{f},\"loop\":{l}}}",
                    quote(n)
                )
            })
            .collect();
        let list = |v: &[String]| v.iter().map(|s| quote(s)).collect::<Vec<_>>().join(",");
        format!(
            "{{\"ok\":{},\"file\":{},\"bytes\":{},\"vertices\":{},\"triangles\":{},\"joints\":{},\"blended_vertices\":{},\"slot_vertices\":{},\"clips\":[{}],\"size\":[{},{},{}],\"min\":[{},{},{}],\"radius\":{},\"n64_draw_us\":{},\"n64_frame_percent\":{:.1},\"warnings\":[{}],\"errors\":[{}]}}",
            self.errors.is_empty(),
            quote(&self.file),
            self.bytes,
            self.verts,
            self.tris,
            self.joints,
            self.blended,
            self.slots.map_or("null".into(), |c| format!("{c:?}")),
            clips.join(","),
            self.size[0],
            self.size[1],
            self.size[2],
            self.min[0],
            self.min[1],
            self.min[2],
            self.radius,
            n64_draw_us(self.verts, self.tris),
            n64_draw_us(self.verts, self.tris) as f32 / FRAME_30FPS_US as f32 * 100.0,
            list(&self.warnings),
            list(&self.errors),
        )
    }

    pub fn text(&self) -> String {
        let mut s = format!("{}\n", self.file);
        s += &format!(
            "  {} triangles, {} vertices, {} KB baked\n",
            self.tris,
            self.verts,
            self.bytes / 1024
        );
        s += &format!(
            "  size {:.3} × {:.3} × {:.3} m (min y {:.3}), bounding radius {:.3}\n",
            self.size[0], self.size[1], self.size[2], self.min[1], self.radius
        );
        let us = n64_draw_us(self.verts, self.tris);
        s += &format!(
            "  N64 cost estimate: ~{:.1} ms per draw ({:.0}% of a 30 fps frame; \
             trino_core::perf::N64_COST)\n",
            us as f32 / 1000.0,
            us as f32 / FRAME_30FPS_US as f32 * 100.0
        );
        if self.joints > 0 && self.blended > 0 {
            s += &format!(
                "  smooth skeleton: {} joint(s), {} vertices blend two joints\n",
                self.joints, self.blended
            );
        } else if self.joints > 0 {
            s += &format!("  rigid skeleton: {} joint(s)\n", self.joints);
        }
        if let Some(c) = self.slots {
            s += &format!(
                "  color slots: {} vertices keep their color; slots 1-4 have {} / {} / {} / {} \
                 (recolored per draw by ModelParams::palette)\n",
                c[0], c[1], c[2], c[3], c[4]
            );
        }
        for (n, d, f, l) in &self.clips {
            s += &format!(
                "  clip `{n}`: {d:.2} s, {f} frames, {}\n",
                if *l { "loops" } else { "plays once" }
            );
        }
        for w in &self.warnings {
            s += &format!("  warning: {w}\n");
        }
        for e in &self.errors {
            s += &format!("  error: {e}\n");
        }
        if self.errors.is_empty() {
            s += "  OK — bakes for PC, N64 and 3DS\n";
        }
        s
    }
}

pub fn inspect(path: &Path, opts: &ModelOptions) -> Result<Report, String> {
    let blob = bake_model(path, opts)?;
    let mesh = Mesh::from_tmdl(&blob).map_err(|e| format!("baked blob invalid: {e:?}"))?;
    let mut min = [f32::MAX; 3];
    let mut max = [f32::MIN; 3];
    for i in 0..mesh.vertex_count {
        let p = mesh.position(i);
        for (k, v) in [p.x, p.y, p.z].into_iter().enumerate() {
            min[k] = min[k].min(v);
            max[k] = max[k].max(v);
        }
    }
    let names = clip_names(path);
    let mut clips = Vec::new();
    for n in 0..mesh.clip_count() {
        let c = mesh.clip(n).unwrap();
        let name = names
            .iter()
            .find(|s| trino_core::ClipId::from_name(s) == c.id)
            .cloned()
            .unwrap_or_else(|| format!("{:#010x}", c.id.0));
        clips.push((name, c.duration(), c.frame_count, c.looping));
    }
    let tris = mesh.index_count / 3;
    let mut warnings = Vec::new();
    let errors = Vec::new();
    // The N64 budget is CPU time, not a triangle count: the calibrated
    // cost model says what one draw costs (Caps::max_tris_per_frame is the
    // rasterizer's cap, which the T&L never gets near).
    let us = n64_draw_us(mesh.vertex_count, tris);
    if us > FRAME_30FPS_US / 4 {
        warnings.push(format!(
            "one draw costs ~{:.1} ms on the N64 ({:.0}% of a 30 fps frame, calibrated \
             estimate) — decimate it, or add a low-poly `lod` level for distance",
            us as f32 / 1000.0,
            us as f32 / FRAME_30FPS_US as f32 * 100.0
        ));
    }
    if min[1] < -0.01 && max[1] > 0.01 && min[1].abs() > 0.25 * (max[1] - min[1]) {
        warnings.push(format!(
            "origin is not at the base (min y = {:.2}): props and characters usually stand on y = 0",
            min[1]
        ));
    }
    if mesh.joint_count() > 0 && mesh.clip_count() == 0 {
        warnings.push("skeleton without clips — did the export include animations?".into());
    }
    if !names.is_empty() && mesh.clip_count() == 0 {
        warnings
            .push("the file names animations but none baked (do they target scene nodes?)".into());
    }
    let (_, radius) = mesh.bounds().unwrap_or((trino_core::Vec3::ZERO, 0.0));
    Ok(Report {
        file: path.display().to_string(),
        bytes: blob.len(),
        verts: mesh.vertex_count,
        tris,
        joints: mesh.joint_count(),
        blended: (0..mesh.vertex_count)
            .filter(|&i| mesh.blend_of(i).is_some())
            .count(),
        slots: mesh.has_slots().then(|| {
            let mut c = [0; 5];
            for i in 0..mesh.vertex_count {
                c[mesh.slot(i) as usize] += 1;
            }
            c
        }),
        clips,
        size: [max[0] - min[0], max[1] - min[1], max[2] - min[2]],
        min,
        radius,
        warnings,
        errors,
    })
}
