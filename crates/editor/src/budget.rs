//! Live console budget for the Game view: what the last frame cost against
//! the selected profile's `Caps` (N64 / 3DS / PC). Pure data here; the
//! viewport draws it as an overlay (green < 75%, amber < 100%, red over).

use trino_core::perf::{FRAME_30FPS_US, N64_COST};
use trino_core::{Caps, FrameStats};
use trino_scene::Target;
use trino_scene::budget::FrameBudget;

/// The scene camera's frame on every target (`trino_scene::budget`, what
/// `cargo xtask scene budget` prints), in [`BUDGET_TARGETS`] order.
pub type Budgets = [FrameBudget; 6];

/// The targets of [`Budgets`]: the N64, the 3DS, the PC, the Web, then the
/// consoles' expanded tiers (issue #54: the Expansion Pak, the New 3DS).
pub const BUDGET_TARGETS: [Target; 6] = [
    Target::N64,
    Target::N3ds,
    Target::Pc,
    Target::Web,
    Target::N64Expanded,
    Target::N3dsNew,
];

/// The budget of `target` in `budgets`.
pub fn budget_of(budgets: &Budgets, target: Target) -> &FrameBudget {
    let i = BUDGET_TARGETS.iter().position(|t| *t == target).unwrap();
    &budgets[i]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Level {
    Ok,
    Near,
    Over,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Row {
    pub label: &'static str,
    pub used: u64,
    pub limit: u64,
    pub unit: &'static str,
    pub level: Level,
    pub hint: &'static str,
}

impl Row {
    pub fn fraction(&self) -> f32 {
        if self.limit == 0 {
            0.0
        } else {
            (self.used as f64 / self.limit as f64) as f32
        }
    }
}

fn level(used: u64, limit: u64) -> Level {
    if limit == u64::MAX || limit == 0 {
        return Level::Ok;
    }
    if used > limit {
        Level::Over
    } else if used * 4 > limit * 3 {
        Level::Near
    } else {
        Level::Ok
    }
}

/// What the Game view drew for entities with LOD levels this frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LodCounts {
    /// Still on their full mesh (closer than the first level).
    pub full: u32,
    /// On a lighter mesh level.
    pub light: u32,
    /// Drawn as impostors (one billboard each).
    pub impostor: u32,
    /// Culled by a level with nothing to draw.
    pub culled: u32,
}

impl LodCounts {
    pub fn total(&self) -> u32 {
        self.full + self.light + self.impostor + self.culled
    }

    /// `LOD 3 full · 5 light · 12 impostor · 2 culled` (None without LOD).
    pub fn summary(&self) -> Option<String> {
        (self.total() > 0).then(|| {
            format!(
                "LOD {} full · {} light · {} imp · {} cull",
                self.full, self.light, self.impostor, self.culled
            )
        })
    }
}

/// Budget rows for one frame. Unlimited caps (`u32::MAX`, the PC profile)
/// never warn. `lod` adds the impostors' cost (billboards are not T&L
/// work) to the N64 time.
pub fn rows(stats: &FrameStats, caps: &Caps, lod: &LodCounts) -> Vec<Row> {
    let lim = |v: u32| if v == u32::MAX { u64::MAX } else { v as u64 };
    let mut out = vec![
        Row {
            label: "Triangles",
            used: stats.tris as u64,
            limit: lim(caps.max_tris_per_frame),
            unit: "",
            level: level(stats.tris as u64, lim(caps.max_tris_per_frame)),
            hint: "3D triangles after culling/clipping/subdivision, against the rasterizer's \
                   cap (Caps::max_tris_per_frame). On the N64 the CPU time binds far earlier: \
                   see N64 3D time",
        },
        Row {
            label: "Sprites",
            used: stats.sprites as u64,
            limit: lim(caps.max_sprites_per_frame),
            unit: "",
            level: level(stats.sprites as u64, lim(caps.max_sprites_per_frame)),
            hint: "sprite draws incl. text glyphs (Caps::max_sprites_per_frame)",
        },
        Row {
            label: "Texture",
            used: stats.max_texture_bytes as u64,
            limit: lim(caps.texture_memory_bytes),
            unit: "B",
            level: level(
                stats.max_texture_bytes as u64,
                lim(caps.texture_memory_bytes),
            ),
            hint: "largest single texture load, worst case 16-bit texels (CI4/CI8 \
                   use less — the PC cannot know the console format)",
        },
    ];
    out.push(Row {
        label: "Models",
        used: stats.models as u64,
        limit: u64::MAX,
        unit: "",
        level: Level::Ok,
        hint: "draw_model calls that reached the T&L (each costs CPU on consoles)",
    });
    // Either N64 tier: the Expansion Pak adds RAM, not CPU time.
    if caps.base() == Caps::N64 {
        // The triangle cap says little about the N64's CPU T&L; the
        // calibrated cost model turns this frame's deterministic T&L work
        // into emulated N64 time (3D only: T&L + sort + submit).
        let us = (N64_COST.estimate_us(&stats.tnl, stats.models)
            + N64_COST.billboards_us(lod.impostor)) as u64;
        let limit = FRAME_30FPS_US as u64;
        out.push(Row {
            label: "N64 3D time",
            used: us,
            limit,
            unit: "us",
            level: level(us, limit),
            hint: "estimated N64 time of this frame's 3D (T&L + sort + submit, plus \
                   ~44 µs per impostor billboard) vs one 30 fps frame — \
                   trino_core::perf::N64_COST, calibrated on ares",
        });
    }
    if lod.impostor > 0 {
        out.push(Row {
            label: "Impostors",
            used: lod.impostor as u64,
            limit: u64::MAX,
            unit: "",
            level: Level::Ok,
            hint: "LOD levels drawn as impostors: 2 textured triangles each (on the N64 \
                   they are CPU-submitted — check the N64 3D time)",
        });
    }
    out
}

/// Put the scene budget's N64 time (CPU vs RSP, `trino_scene::budget`) in
/// the "N64 3D time" row, so the HUD's two N64 figures agree: the row from
/// the drawn frame alone counts every draw as CPU work.
pub fn use_rsp_split(rows: &mut [Row], n64: &trino_scene::budget::FrameBudget) {
    if let Some(r) = rows.iter_mut().find(|r| r.label == "N64 3D time") {
        r.used = n64.n64_frame_us() as u64;
        r.level = level(r.used, r.limit);
        r.hint = "estimated N64 time of the scene camera's frame: the slower of the CPU \
                  (T&L + sort + submit of the draws off the RSP fast path, ~44 µs per \
                  billboard) and the RSP (its draws' triangles) vs one 30 fps frame — \
                  trino_scene::budget, calibrated on ares";
    }
}

/// The Game view's per-platform lines for the scene camera's frame: the
/// N64 time (CPU vs RSP), how many draws take the RSP and the most common
/// reason the rest don't, the 3DS triangles and GPU split, the PC native
/// profile's GPU T&L split (issue #51), the Web's calls (issue #47), and
/// draws sent for nothing. Each
/// with its level (the N64 time against a 30 fps frame). The console the
/// Game view shows (`shown`) is read on its hardware tier (issue #54):
/// `N64+Pak ≈…` and `New 3DS …`; any other console on its base tier.
pub fn platform_lines_on(
    budgets: &Budgets,
    tmem: &dyn Fn(trino_core::SpriteId) -> Option<u32>,
    shown: Target,
) -> Vec<(String, Level)> {
    let [n64, n3ds, pc, web, n64x, n3ds_new] = budgets;
    let (n64, n64_label) = if shown == Target::N64Expanded {
        (n64x, Target::N64Expanded.label())
    } else {
        (n64, "N64")
    };
    let (n3ds, n3ds_label) = if shown == Target::N3dsNew {
        (n3ds_new, Target::N3dsNew.label())
    } else {
        (n3ds, "3DS")
    };
    let pct = n64.n64_frame_percent();
    let level = if pct > 100.0 {
        Level::Over
    } else if pct > 75.0 {
        Level::Near
    } else {
        Level::Ok
    };
    let mut out = vec![(
        format!(
            "{n64_label} ≈{:.1} ms · CPU {:.1} · RSP {:.1}",
            n64.n64_frame_us() as f32 / 1000.0,
            n64.n64_cpu_us() as f32 / 1000.0,
            n64.n64_rsp_us() as f32 / 1000.0
        ),
        level,
    )];
    let mut rsp = format!("RSP {}/{} draws", n64.rsp_draws, n64.draws);
    if let Some((r, k)) = n64.off_rsp.first() {
        rsp.push_str(&format!(" · {k} {}", r.label()));
    }
    out.push((rsp, Level::Ok));
    let mut gpu = format!(
        "{n3ds_label} {} tris · GPU {}/{} draws",
        n3ds.tris(),
        n3ds.gpu_draws,
        n3ds.draws
    );
    if let Some((r, k)) = n3ds.off_gpu.first() {
        gpu.push_str(&format!(" · {k} {}", r.label()));
    }
    out.push((gpu, Level::Ok));
    out.push((trino_scene::budget::pc_gpu_line(pc), Level::Ok));
    // The Web (issue #47): what a browser's GPU API is asked for, and a
    // warning past a phone's budget.
    out.push((trino_scene::budget::web_line(web), Level::Ok));
    for w in trino_scene::budget::web_warnings(web) {
        out.push((w, Level::Near));
    }
    let wasted = n64.wasted.len().max(n3ds.wasted.len());
    if wasted > 0 {
        out.push((format!("{wasted} draws sent but unseen"), Level::Near));
    }
    // Water (ADR 0015): the N64 tier's triangles, combiner cycles, TMEM.
    for line in trino_scene::budget::water_lines(n64, tmem) {
        let level = if line.contains("over TMEM") || line.contains("palette layer") {
            Level::Over
        } else {
            Level::Ok
        };
        out.push((line, level));
    }
    // Shadows (ADR 0025): each tier's casters, a cast past the N64's
    // planar budget warned.
    for (b, label) in [(n64, n64_label), (n3ds, n3ds_label), (pc, "PC")] {
        if let Some(line) = trino_scene::budget::shadow_line(b) {
            let level = if b.shadows.over_budget > 0 {
                Level::Near
            } else {
                Level::Ok
            };
            out.push((format!("{label} {line}"), level));
        }
    }
    out
}

/// A console's two hardware tiers side by side (issue #54, ADR 0020): the
/// Game view's HUD and the Inspector's platform budget show what the scene
/// costs on the base console and on its expanded tier.
#[derive(Clone, Debug, PartialEq)]
pub struct TierTable {
    /// The base console and its expanded tier (`N64` and `N64+Pak`, `3DS`
    /// and `New 3DS`).
    pub targets: [Target; 2],
    /// `(label, base, expanded)`: RAM (the scene's models of the tier's
    /// RAM), CPU, the frame (the N64's time, the 3DS's triangles), draws,
    /// and the advisory crowd and particle scales where they differ.
    pub rows: Vec<(&'static str, String, String)>,
    /// The models' share of each tier's RAM, 0..=1 (`Caps::ram_bytes`).
    pub ram: [f32; 2],
}

/// The [`TierTable`] of `console`'s tiers (either tier names it); `None`
/// for the PC and the Web, which have one.
pub fn tier_table(budgets: &Budgets, console: Target) -> Option<TierTable> {
    let base = console.base();
    let expanded = base.expanded()?;
    let b = [budget_of(budgets, base), budget_of(budgets, expanded)];
    let caps = [base, expanded].map(trino_scene::budget::caps_of);
    let both = |f: &dyn Fn(usize) -> String| (f(0), f(1));
    let mut rows = Vec::new();
    let mut row = |label, (a, x): (String, String)| rows.push((label, a, x));
    row(
        "RAM",
        both(&|i| {
            format!(
                "{} KB / {} MB",
                b[i].model_bytes.div_ceil(1024),
                caps[i].ram_bytes >> 20
            )
        }),
    );
    row(
        "CPU",
        both(&|i| format!("{}×{} MHz", caps[i].cpu_cores, caps[i].cpu_mhz)),
    );
    if base == Target::N64 {
        row(
            "Frame",
            both(&|i| format!("≈{:.1} ms", b[i].n64_frame_us() as f32 / 1000.0)),
        );
    } else {
        row("Tris", both(&|i| b[i].tris().to_string()));
    }
    row("Draws", both(&|i| b[i].draws.to_string()));
    if caps[0].crowd_percent != caps[1].crowd_percent {
        row("Crowds", both(&|i| format!("×{}", caps[i].crowd_scale())));
    }
    if caps[0].particle_percent != caps[1].particle_percent {
        row(
            "Particles",
            both(&|i| format!("×{}", caps[i].particle_scale())),
        );
    }
    let ram = [0, 1].map(|i| b[i].model_bytes as f32 / caps[i].ram_bytes as f32);
    Some(TierTable {
        targets: [base, expanded],
        rows,
        ram,
    })
}

/// The Game view's measured Web line (issue #47): what its renderer just
/// drew — GPU draw calls and pipeline changes, the calls a browser pays
/// for in JavaScript — and the GPU memory of its textures and resident
/// meshes (the Web's textures are KTX2 transcoded; this is the PC's view of
/// the same project). The estimate from the scene camera is `web_line`.
pub fn web_measured_line(
    times: &trino_platform_pc::PcFrameTimes,
    textures: u64,
    meshes: u64,
) -> String {
    format!(
        "Web drawn {} calls · {} {} · {} VRAM",
        times.draw_calls,
        times.pipeline_switches,
        if times.pipeline_switches == 1 {
            "switch"
        } else {
            "switches"
        },
        crate::textures::bytes(textures + meshes)
    )
}

/// The PC's and the Web's quality line (issue #55): the preset the Game
/// view draws with, the 3D's live scale (dynamic resolution moves it), MSAA
/// and the AO cap.
pub fn quality_line(q: &trino_core::QualitySettings, scale: f32) -> String {
    let dynres = match q.dynamic_resolution {
        Some(d) => format!(" (dynres {:.2}-{:.2})", d.min_scale, d.max_scale),
        None => String::new(),
    };
    format!(
        "Quality {} · 3D {:.2}×{dynres} · MSAA {} · AO {}",
        q.preset.label(),
        scale,
        if q.msaa > 1 { "4×" } else { "off" },
        q.ao.key()
    )
}

/// The HUD's assets RAM line (issue #56, ADR 0022 §6): what `loaded` (a
/// scene's groups) holds of the RAM `plan`'s tier leaves for assets, as the
/// Memory tab's bar says it: `N64 assets 289 KB / 1.8 MB RAM (15%)` (`label`:
/// the target's). Near past 85 %, over past the budget (strict mode would
/// panic).
pub fn assets_ram_line(
    label: &str,
    plan: &trino_asset_pipeline::memory::MemoryPlan,
    loaded: &[&str],
) -> (String, Level) {
    use trino_asset_pipeline::memory::show_bytes;
    let used = plan.resident(loaded);
    let budget = plan.budget();
    let level = if used > budget {
        Level::Over
    } else if used * 100 > budget * 85 {
        Level::Near
    } else {
        Level::Ok
    };
    let line = format!(
        "{label} assets {} / {} RAM ({}%)",
        show_bytes(used),
        show_bytes(budget),
        (used * 100).checked_div(budget).unwrap_or(100)
    );
    (line, level)
}

/// The Game view budget panel's width for its widest text line (`widest`
/// px) in a view `view` px wide: the text plus its margins, at least
/// [`HUD_MIN_WIDTH`], and never past the view (10 px off each side) unless
/// the view is narrower than the minimum.
pub fn hud_width(widest: f32, view: f32) -> f32 {
    (widest + 16.0)
        .max(HUD_MIN_WIDTH)
        .min((view - 20.0).max(HUD_MIN_WIDTH))
}

/// The budget panel's narrowest width (its rows' labels and values).
pub const HUD_MIN_WIDTH: f32 = 210.0;

#[cfg(test)]
mod tests {
    #[test]
    fn the_quality_line_shows_the_preset_and_the_live_scale() {
        let low = super::quality_line(&trino_core::QualitySettings::LOW, 0.8125);
        assert_eq!(
            low,
            "Quality Low · 3D 0.81× (dynres 0.50-1.00) · MSAA off · AO off"
        );
        let high = super::quality_line(&trino_core::QualitySettings::HIGH, 1.0);
        assert_eq!(high, "Quality High · 3D 1.00× · MSAA 4× · AO high");
    }

    #[test]
    fn the_web_line_reads_the_renderers_counters() {
        let times = trino_platform_pc::PcFrameTimes {
            draw_calls: 42,
            pipeline_switches: 7,
            ..Default::default()
        };
        assert_eq!(
            super::web_measured_line(&times, 1536 * 1024, 512 * 1024),
            "Web drawn 42 calls · 7 switches · 2.0 MB VRAM"
        );
    }

    #[test]
    fn the_hud_grows_to_its_longest_line_within_the_view() {
        use super::{HUD_MIN_WIDTH, hud_width};
        assert_eq!(hud_width(100.0, 800.0), HUD_MIN_WIDTH, "short lines");
        assert_eq!(hud_width(260.0, 800.0), 276.0, "a long reason fits");
        assert_eq!(hud_width(600.0, 400.0), 380.0, "never past the view");
        assert_eq!(hud_width(600.0, 150.0), HUD_MIN_WIDTH, "a tiny view");
    }

    #[test]
    fn the_assets_ram_line_follows_the_memory_plan() {
        use super::{Level, assets_ram_line};
        let assets = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets");
        let mut plan = trino_asset_pipeline::memory::plan(
            &assets,
            trino_asset_pipeline::Platform::N64,
            trino_core::HwTier::Base,
        )
        .unwrap();
        let (line, level) = assets_ram_line("N64", &plan, &["boot"]);
        assert_eq!(level, Level::Ok, "{line}");
        assert!(
            line.starts_with("N64 assets ") && line.contains(" / 1.8 MB RAM ("),
            "{line}"
        );
        // A game that keeps more for itself leaves less for the assets.
        let boot = plan.resident(&["boot"]);
        plan.game = plan.ram - plan.reserve - boot - boot / 10;
        assert_eq!(assets_ram_line("N64", &plan, &["boot"]).1, Level::Near);
        plan.game = plan.ram - plan.reserve - boot / 2;
        let (line, level) = assets_ram_line("N64", &plan, &["boot"]);
        assert_eq!(level, Level::Over);
        assert!(line.ends_with("(200%)"), "{line}");
    }

    use super::*;

    #[test]
    fn platform_lines_name_the_rsp_reason_and_unseen_draws() {
        use trino_core::RspReject;
        use trino_scene::{Target, budget::FrameBudget};
        let n64 = FrameBudget {
            draws: 5,
            rsp_draws: 2,
            rsp_tris: 100,
            cpu_draws: 3,
            off_rsp: vec![(RspReject::Fog, 2), (RspReject::Skinned, 1)],
            wasted: vec!["far".into()],
            target: Target::N64,
            ..Default::default()
        };
        let n3ds = FrameBudget {
            draws: 5,
            billboards: 1,
            gpu_draws: 4,
            off_gpu: vec![(trino_core::render3d::GpuReject::Flat, 1)],
            target: Target::N3ds,
            ..Default::default()
        };
        let pc = FrameBudget {
            draws: 5,
            pc_gpu_draws: 3,
            pc_gpu_tris: 240,
            off_pc_gpu: vec![
                (trino_core::PcGpuReject::Translucent, 1),
                (trino_core::PcGpuReject::Water, 1),
            ],
            target: Target::Pc,
            ..Default::default()
        };
        let web = FrameBudget {
            target: Target::Web,
            web: trino_scene::budget::WebCalls {
                calls: 4,
                pipelines: 2,
                switches: 3,
            },
            ..pc.clone()
        };
        let lines = platform_lines(&six([n64.clone(), n3ds, pc.clone(), web.clone()]), &|_| {
            None
        });
        let text: Vec<&str> = lines.iter().map(|(t, _)| t.as_str()).collect();
        assert!(text[0].starts_with("N64 ≈"), "{text:?}");
        assert_eq!(text[1], "RSP 2/5 draws · 2 fog needing splits");
        assert_eq!(text[2], "3DS 2 tris · GPU 4/5 draws · 1 flat shading");
        assert_eq!(text[3], "PC GPU 3/5 draws · 240 tris · 1 translucent");
        assert_eq!(text[4], "Web 4 calls · 2 pipelines · 3 switches");
        assert_eq!(lines[5], ("1 draws sent but unseen".into(), Level::Near));
        // Past a phone's budget the Web warns.
        let busy = FrameBudget {
            web: trino_scene::budget::WebCalls {
                calls: 999,
                pipelines: 4,
                switches: 998,
            },
            ..web.clone()
        };
        let warned = platform_lines(&six([n64.clone(), n64.clone(), pc.clone(), busy]), &|_| {
            None
        });
        assert!(
            warned
                .iter()
                .any(|(t, l)| t.contains("CPU-bound on mobile") && *l == Level::Near),
            "{warned:?}"
        );
        // A light frame is green; one past 33.3 ms is red.
        assert_eq!(lines[0].1, Level::Ok);
        let heavy = FrameBudget {
            billboards: 1000,
            ..n64.clone()
        };
        assert_eq!(
            platform_lines(&six([heavy, n64.clone(), pc.clone(), web]), &|_| None)[0].1,
            Level::Over
        );
        // The HUD's N64 time row takes the CPU/RSP split.
        let mut r = rows(&FrameStats::default(), &Caps::N64, &LodCounts::default());
        use_rsp_split(&mut r, &n64);
        let time = r.iter().find(|r| r.label == "N64 3D time").unwrap();
        assert_eq!(time.used, n64.n64_frame_us() as u64);
        let mut pc = rows(&FrameStats::default(), &Caps::PC, &LodCounts::default());
        use_rsp_split(&mut pc, &n64);
        assert_eq!(
            pc,
            rows(&FrameStats::default(), &Caps::PC, &LodCounts::default())
        );
        // Nothing unseen: no warning line.
        let clean = FrameBudget {
            wasted: vec![],
            ..n64.clone()
        };
        assert_eq!(
            platform_lines(
                &six([clean.clone(), clean.clone(), clean.clone(), clean]),
                &|_| None
            )
            .len(),
            5
        );
    }

    /// The lines with every console on its base tier.
    fn platform_lines(
        budgets: &Budgets,
        tmem: &dyn Fn(trino_core::SpriteId) -> Option<u32>,
    ) -> Vec<(String, Level)> {
        platform_lines_on(budgets, tmem, Target::Pc)
    }

    /// The four builds' budgets, the consoles' tiers as their console.
    fn six(b: [FrameBudget; 4]) -> Budgets {
        let [n64, n3ds, pc, web] = b;
        let tier = |b: &FrameBudget, t| FrameBudget {
            target: t,
            ..b.clone()
        };
        let (x, new) = (
            tier(&n64, Target::N64Expanded),
            tier(&n3ds, Target::N3dsNew),
        );
        [n64, n3ds, pc, web, x, new]
    }

    /// The Game view on a tier reads that tier's frame in its console's
    /// lines; the other console keeps its base tier (issue #54).
    #[test]
    fn platform_lines_follow_the_game_views_tier() {
        let n64 = FrameBudget {
            draws: 2,
            rsp_draws: 2,
            target: Target::N64,
            ..Default::default()
        };
        let n3ds = FrameBudget {
            draws: 2,
            gpu_draws: 2,
            target: Target::N3ds,
            ..Default::default()
        };
        let mut b = six([n64, n3ds, FrameBudget::default(), FrameBudget::default()]);
        b[4].draws = 9;
        b[4].rsp_draws = 9;
        b[5].draws = 7;
        b[5].gpu_draws = 7;
        let text = |shown| -> Vec<String> {
            platform_lines_on(&b, &|_| None, shown)
                .into_iter()
                .map(|(t, _)| t)
                .collect()
        };
        let base = text(Target::N64);
        assert!(base[0].starts_with("N64 ≈"), "{base:?}");
        assert_eq!(base[1], "RSP 2/2 draws");
        assert!(base[2].starts_with("3DS ") && base[2].ends_with("GPU 2/2 draws"));
        assert_eq!(base, text(Target::Pc));
        let pak = text(Target::N64Expanded);
        assert!(pak[0].starts_with("N64+Pak ≈"), "{pak:?}");
        assert_eq!(pak[1], "RSP 9/9 draws");
        assert_eq!(pak[2], base[2], "the 3DS stays on its base tier");
        let new = text(Target::N3dsNew);
        assert_eq!(new[1], "RSP 2/2 draws");
        assert!(new[2].starts_with("New 3DS ") && new[2].ends_with("GPU 7/7 draws"));
    }

    /// Shadows (ADR 0025): a line per console and the PC with its tier and
    /// casters, warned when the N64's planar budget leaves some without;
    /// none for a scene without any.
    #[test]
    fn platform_lines_list_each_tiers_shadows() {
        use trino_core::ShadowTier;
        use trino_scene::budget::ShadowBudget;
        let with = |target, s: ShadowBudget| FrameBudget {
            target,
            shadows: s,
            ..Default::default()
        };
        let none = six([
            with(Target::N64, ShadowBudget::default()),
            with(Target::N3ds, ShadowBudget::default()),
            with(Target::Pc, ShadowBudget::default()),
            FrameBudget::default(),
        ]);
        let lines = platform_lines_on(&none, &|_| None, Target::N64);
        assert!(lines.iter().all(|(t, _)| !t.contains("shadows")));
        let b = six([
            with(
                Target::N64,
                ShadowBudget {
                    tier: ShadowTier::Blob,
                    casters: 5,
                    blobs: 2,
                    planar: 2,
                    over_budget: 1,
                    receivers: 1,
                    ..Default::default()
                },
            ),
            with(
                Target::N3ds,
                ShadowBudget {
                    tier: ShadowTier::Planar,
                    casters: 5,
                    planar: 5,
                    receivers: 1,
                    ..Default::default()
                },
            ),
            with(
                Target::Pc,
                ShadowBudget {
                    tier: ShadowTier::Map,
                    casters: 5,
                    mapped: 5,
                    receivers: 1,
                    ..Default::default()
                },
            ),
            FrameBudget::default(),
        ]);
        let lines = platform_lines_on(&b, &|_| None, Target::N64);
        let line = |p: &str| {
            lines
                .iter()
                .find(|(t, _)| t.starts_with(p))
                .unwrap_or_else(|| panic!("{p}: {lines:?}"))
                .clone()
        };
        let (n64, level) = line("N64 shadows: blob — 5 casters: 2 blobs, 2 planar");
        assert!(n64.contains("1 past the planar budget"), "{n64}");
        assert!(n64.contains("of N64 CPU"), "the blobs' cost: {n64}");
        assert_eq!(level, Level::Near);
        assert_eq!(line("3DS shadows: planar").1, Level::Ok);
        assert!(line("PC shadows: map").0.contains("5 in the map"));
    }

    /// The HUD's and the Inspector's tier columns (issue #54): the base
    /// console and its expanded tier side by side, from either tier.
    #[test]
    fn tier_tables_put_a_consoles_two_tiers_side_by_side() {
        let models = |t, model_bytes, draws| FrameBudget {
            target: t,
            model_bytes,
            draws,
            ..Default::default()
        };
        let budgets = [
            models(Target::N64, 412 * 1024, 10),
            models(Target::N3ds, 2 << 20, 10),
            models(Target::Pc, 0, 10),
            models(Target::Web, 0, 10),
            models(Target::N64Expanded, 900 * 1024, 14),
            models(Target::N3dsNew, 2 << 20, 12),
        ];
        assert_eq!(budget_of(&budgets, Target::N3dsNew).draws, 12);
        let n64 = tier_table(&budgets, Target::N64).unwrap();
        assert_eq!(n64, tier_table(&budgets, Target::N64Expanded).unwrap());
        assert_eq!(n64.targets, [Target::N64, Target::N64Expanded]);
        let row = |t: &TierTable, l: &str| {
            let r = t.rows.iter().find(|r| r.0 == l).unwrap();
            (r.1.clone(), r.2.clone())
        };
        assert_eq!(
            row(&n64, "RAM"),
            ("412 KB / 4 MB".into(), "900 KB / 8 MB".into())
        );
        assert_eq!(row(&n64, "CPU"), ("1×93 MHz".into(), "1×93 MHz".into()));
        assert_eq!(row(&n64, "Draws"), ("10".into(), "14".into()));
        assert!(n64.rows.iter().any(|r| r.0 == "Frame"));
        assert!(
            n64.rows.iter().all(|r| r.0 != "Crowds"),
            "the Pak adds RAM only"
        );
        assert!((n64.ram[0] - 412.0 / 4096.0).abs() < 1e-4);
        let n3ds = tier_table(&budgets, Target::N3ds).unwrap();
        assert_eq!(n3ds.targets, [Target::N3ds, Target::N3dsNew]);
        assert_eq!(row(&n3ds, "CPU"), ("1×268 MHz".into(), "2×804 MHz".into()));
        assert_eq!(row(&n3ds, "Crowds"), ("×1".into(), "×2".into()));
        assert_eq!(row(&n3ds, "Particles"), ("×1".into(), "×2".into()));
        assert!(n3ds.rows.iter().any(|r| r.0 == "Tris"));
        assert_eq!(tier_table(&budgets, Target::Pc), None);
        assert_eq!(tier_table(&budgets, Target::Web), None);
    }

    #[test]
    fn n64_profile_estimates_3d_time() {
        let stats = FrameStats {
            models: 1,
            tnl: trino_core::TnlStats {
                verts: 1790,
                tris_in: 1068,
                emitted: 552,
                ..Default::default()
            },
            ..FrameStats::default()
        };
        let r = rows(&stats, &Caps::N64, &LodCounts::default());
        let time = r.iter().find(|r| r.label == "N64 3D time").unwrap();
        // KayKit's original 1068-tri door alone: about two 30 fps frames.
        assert_eq!(time.level, Level::Over);
        assert!(time.used > 50_000, "{}", time.used);
        assert!(
            rows(&stats, &Caps::PC, &LodCounts::default())
                .iter()
                .all(|r| r.label != "N64 3D time")
        );
    }

    #[test]
    fn levels_follow_the_n64_caps() {
        let stats = FrameStats {
            tris: 3500,
            sprites: 10,
            models: 40,
            max_texture_bytes: 8192,
            ..FrameStats::default()
        };
        let r = rows(&stats, &Caps::N64, &LodCounts::default());
        assert_eq!(r[0].level, Level::Near, "3500 / 4000 tris");
        assert_eq!(r[1].level, Level::Ok);
        assert_eq!(r[2].level, Level::Over, "8 KB texture vs 4 KB TMEM");
        assert!((r[0].fraction() - 0.875).abs() < 1e-6);
    }

    #[test]
    fn impostors_add_their_billboard_cost_to_the_n64_time() {
        let stats = FrameStats::default();
        let lod = LodCounts {
            full: 1,
            light: 2,
            impostor: 10,
            culled: 3,
        };
        let r = rows(&stats, &Caps::N64, &lod);
        let time = r.iter().find(|r| r.label == "N64 3D time").unwrap();
        assert_eq!(time.used, 440, "10 billboards at 44 µs");
        assert_eq!(r.iter().find(|r| r.label == "Impostors").unwrap().used, 10);
        assert_eq!(lod.total(), 16);
        assert_eq!(
            lod.summary().unwrap(),
            "LOD 1 full · 2 light · 10 imp · 3 cull"
        );
        assert!(LodCounts::default().summary().is_none());
    }

    #[test]
    fn pc_profile_never_warns() {
        let stats = FrameStats {
            tris: 1_000_000,
            sprites: 100_000,
            models: 1,
            max_texture_bytes: 1 << 30,
            ..FrameStats::default()
        };
        assert!(
            rows(&stats, &Caps::PC, &LodCounts::default())
                .iter()
                .all(|r| r.level == Level::Ok)
        );
    }
}
