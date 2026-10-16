//! castle64's performance budget on the host: every `castle64::perf` scene
//! through the engine's real software T&L (`trino_core::render3d::
//! tessellate`, the code every console runs), counting the work.
//!
//! The counters are deterministic and identical on every target (same code,
//! same f32 math), so pinning them here pins the consoles' T&L workload: a
//! heavier model, a level that draws more, a depth-split explosion or a
//! culling regression fails `cargo test --workspace` with no emulator.
//! `cargo xtask test n64` then requires the ROM to report the **same**
//! counters and times them in ares against `tests/perf/castle64_n64.toml`.
//!
//! Intentional change (content or engine): rewrite the baseline with
//! `TRINO_BLESS=1 cargo test -p xtask --test perf_budget` (or `cargo xtask
//! test --bless`) and review `tests/perf/castle64_work.toml` in the diff —
//! a number going up is a cost someone pays on the N64.

use std::collections::{BTreeMap, HashMap};
use std::fmt::Write as _;
use std::path::PathBuf;

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, TnlContext, TnlStats, rsp_eligible, rsp_mesh_supported,
};
use trino_core::*;

const BASELINE: &str = "tests/perf/castle64_work.toml";
/// The N64 screen: projection and culling depend on it, and the N64 is
/// the target the baseline is cross-checked on.
const SCREEN: Vec2 = Vec2::new(320.0, 240.0);
const FIELDS: [&str; 9] = [
    "verts",
    "tris_in",
    "clipped",
    "splits",
    "backfaced",
    "emitted",
    "culled",
    "rsp_draws",
    "rsp_tris",
];

struct NullAudio;
impl Audio for NullAudio {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// A renderer that only runs the T&L and counts (no rasterizer).
struct Counting {
    meshes: HashMap<u32, Vec<u8>>,
    camera: Camera3,
    light: Light,
    /// Local lights, per vertex like the N64 (its cap).
    locals: trino_core::LightList,
    caps: Caps,
    total: TnlStats,
    per_model: BTreeMap<u32, (u32, TnlStats)>,
}

impl Renderer for Counting {
    fn caps(&self) -> &Caps {
        &self.caps
    }
    fn begin_frame(&mut self, _: Color) {
        self.locals = trino_core::LightList::EMPTY;
    }
    fn set_local_lights(&mut self, lights: &[trino_core::LocalLight]) {
        self.locals.set(lights, self.caps.max_local_lights);
    }
    fn draw_sprite(&mut self, _: SpriteId, _: Vec2, _: &SpriteParams) {}
    fn set_camera(&mut self, c: &Camera3) {
        self.camera = *c;
    }
    fn set_light(&mut self, l: &Light) {
        self.light = *l;
    }
    fn draw_model(&mut self, m: ModelId, t: &Transform3, mat: Material, p: &ModelParams) {
        self.draw_model_matrix(m, &t.matrix(), mat, p)
    }
    fn draw_model_matrix(&mut self, m: ModelId, x: &Mat34, _: Material, p: &ModelParams) {
        let Some(bytes) = self.meshes.get(&m.0) else {
            return;
        };
        let mesh = Mesh::from_tmdl(bytes).unwrap();
        // The N64 sends eligible draws to its RSP overlay (same rules,
        // trino_core::render3d::rsp_eligible): count those as RSP work, as
        // the ROM does.
        let ctx = TnlContext::new(&self.camera, &self.light, SCREEN)
            .with_local_lights(self.locals.as_slice());
        let s = if rsp_mesh_supported(&mesh) && rsp_eligible(&ctx, &mesh, x, p).is_some() {
            TnlStats {
                rsp_draws: 1,
                rsp_tris: (mesh.index_count / 3) as u32,
                ..TnlStats::default()
            }
        } else {
            let mut scratch = trino_core::TnlScratch::new();
            trino_core::tessellate_with(&ctx, &mut scratch, &mesh, x, p, &mut |_| {})
        };
        self.total.add(&s);
        let e = self.per_model.entry(m.0).or_default();
        e.0 += 1;
        e.1.add(&s);
    }
    fn end_frame(&mut self) {}
}

fn field(t: &TnlStats, name: &str) -> u32 {
    match name {
        "verts" => t.verts,
        "tris_in" => t.tris_in,
        "clipped" => t.clipped,
        "splits" => t.splits,
        "backfaced" => t.backfaced,
        "emitted" => t.emitted,
        "culled" => t.culled,
        "rsp_draws" => t.rsp_draws,
        "rsp_tris" => t.rsp_tris,
        _ => unreachable!(),
    }
}

/// Per-frame figures, integer-averaged exactly like
/// `trino_core::perf::RenderProfile::write_report` (what the N64 prints).
fn per_frame(t: &TnlStats, n: u32) -> TnlStats {
    TnlStats {
        culled: t.culled / n,
        verts: t.verts / n,
        tris_in: t.tris_in / n,
        clipped: t.clipped / n,
        splits: t.splits / n,
        backfaced: t.backfaced / n,
        emitted: t.emitted / n,
        rsp_draws: t.rsp_draws / n,
        rsp_tris: t.rsp_tris / n,
    }
}

/// Model handle -> manifest name, for readable failure reports.
fn model_names(root: &std::path::Path) -> HashMap<u32, String> {
    let manifest = std::fs::read_to_string(root.join("assets/manifest.toml")).unwrap();
    let table: toml::Table = toml::from_str(&manifest).unwrap();
    let mut out = HashMap::new();
    if let Some(models) = table.get("models").and_then(|m| m.as_table()) {
        for name in models.keys() {
            out.insert(
                ModelId::from_path(&format!("models/{name}")).0,
                name.clone(),
            );
        }
    }
    out
}

fn render_baseline(scenes: &[(&str, TnlStats)]) -> String {
    let mut out = String::from(
        "# castle64 T&L work per frame (castle64::perf::SCENES) — deterministic,\n\
         # identical on every target. Checked for equality by\n\
         # xtask/tests/perf_budget.rs (PC) and by `cargo xtask test n64` (the\n\
         # ROM's TRINO_PERF_SCENE counters). Generated: rewrite with\n\
         # `TRINO_BLESS=1 cargo test -p xtask --test perf_budget` and review —\n\
         # a number going up is a cost the N64 pays.\n",
    );
    for (name, t) in scenes {
        let _ = writeln!(out, "\n[{name}]");
        for f in FIELDS {
            let _ = writeln!(out, "{f} = {}", field(t, f));
        }
    }
    out
}

#[test]
fn castle64_tnl_work_matches_the_baseline() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let names = model_names(&root);
    let meshes: HashMap<u32, Vec<u8>> = assets.models.into_iter().map(|m| (m.id, m.tmdl)).collect();

    let n = castle64::perf::MEASURE_FRAMES;
    let mut measured = Vec::new();
    let mut reports = String::new();
    for scene in castle64::perf::SCENES {
        let mut r = Counting {
            meshes: meshes.clone(),
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
            locals: trino_core::LightList::EMPTY,
            caps: Caps::N64,
            total: TnlStats::default(),
            per_model: BTreeMap::new(),
        };
        let mut audio = NullAudio;
        let mut game = castle64::perf::scene_game(scene, SCREEN, &mut audio);
        let idle = InputState::default();
        for _ in 0..n {
            game.update(&idle, &mut audio, castle64::perf::DT);
            game.render(&mut r);
        }
        let t = per_frame(&r.total, n);
        // Most expensive models first (by vertex work), for the report.
        let mut models: Vec<_> = r.per_model.iter().collect();
        models.sort_by_key(|(_, (_, s))| std::cmp::Reverse(s.verts));
        let _ = writeln!(reports, "  {}: {t:?}", scene.name);
        for (id, (calls, s)) in models.iter().take(4) {
            let s = per_frame(s, n);
            let _ = writeln!(
                reports,
                "      {:<22} calls {:>3}  verts {:>5}  tris_in {:>5}  emitted {:>4}  splits {:>4}",
                names.get(id).map_or("?", |s| s.as_str()),
                calls / n,
                s.verts,
                s.tris_in,
                s.emitted,
                s.splits,
            );
        }
        measured.push((scene.name, t));
    }

    let path = root.join(BASELINE);
    let fresh = render_baseline(&measured);
    if std::env::var_os("TRINO_BLESS").is_some() {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, &fresh).unwrap();
        eprintln!("blessed {BASELINE}:\n{reports}");
        return;
    }
    let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "{BASELINE}: {e} — create it with TRINO_BLESS=1 cargo test -p xtask --test perf_budget"
        )
    });
    let baseline: toml::Table = toml::from_str(&text).unwrap();
    let mut diffs = String::new();
    for (name, t) in &measured {
        let Some(want) = baseline.get(*name).and_then(|v| v.as_table()) else {
            let _ = writeln!(diffs, "  {name}: missing from the baseline");
            continue;
        };
        for f in FIELDS {
            let expected = want.get(f).and_then(|v| v.as_integer()).unwrap_or(-1);
            let got = field(t, f) as i64;
            if expected != got {
                let _ = writeln!(diffs, "  {name}.{f}: baseline {expected}, now {got}");
            }
        }
    }
    let stale: Vec<_> = baseline
        .keys()
        .filter(|k| !measured.iter().any(|(n, _)| n == k))
        .collect();
    if !stale.is_empty() {
        let _ = writeln!(diffs, "  scenes no longer measured: {stale:?}");
    }
    assert!(
        diffs.is_empty(),
        "castle64's T&L work changed (per frame, N64 screen):\n{diffs}\n\
         measured now:\n{reports}\n\
         If intentional, bless with `TRINO_BLESS=1 cargo test -p xtask --test perf_budget` \
         and review {BASELINE}; then refresh the N64 ceilings with \
         `cargo xtask test n64 --bless`."
    );
}
