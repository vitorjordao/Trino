//! Asset groups and RAM from the command line (issue #56, ADR 0022): what
//! the editor's Assets panel and Memory tab show, for agents and CI.
//!
//! - `assets report [--platform p] [--tier base|expanded] [--json]`: per
//!   console and tier, the RAM left for assets (`[memory]`), each group's
//!   bytes once loaded and on the medium (from the last bake, when there
//!   is one), and the scenarios: boot, boot plus each group, each scene's
//!   `requires`. Fails when a scenario does not fit, like strict mode.
//! - `assets check [--json]`: every scene's `requires` and cell groups are
//!   declared, every asset a scene draws is in boot or in one of them, and
//!   each scene's groups fit every console tier with all its rooms loaded
//!   (an exterior camera sees every cell).
//! - `assets group <logical> <g1,g2|boot> [--json]`: put an asset in groups
//!   (declaring new ones), the editor's group chips.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use trino_asset_pipeline::memory::{self, MemoryPlan, show_bytes};
use trino_asset_pipeline::{Manifest, Platform, groups};
use trino_core::HwTier;
use trino_scene::{Library, Scene};

const USAGE: &str = "xtask: usage:\n  cargo xtask assets report [--platform n64|3ds|pc|web] \
                     [--tier base|expanded] [--json]\n  cargo xtask assets check [--json]\n  \
                     cargo xtask assets group <sprites/x|models/x|..> <group[,group..]|boot> [--json]";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let rest: Vec<&str> = args.iter().copied().filter(|a| *a != "--json").collect();
    let result = match rest.as_slice() {
        ["report", opts @ ..] => report(root, opts, json),
        ["check"] => check(root, json),
        ["group", logical, groups] => group(root, logical, groups, json),
        _ => Err(USAGE.to_string()),
    };
    match result {
        Ok(true) => ExitCode::SUCCESS,
        Ok(false) => ExitCode::FAILURE,
        Err(e) => {
            eprintln!("error: {e}");
            ExitCode::FAILURE
        }
    }
}

/// The console tiers a report covers by default: every tier with a RAM
/// ceiling.
fn consoles() -> Vec<(Platform, HwTier)> {
    let mut out = Vec::new();
    for p in [Platform::N64, Platform::N3ds] {
        for t in [HwTier::Base, HwTier::Expanded] {
            out.push((p, t));
        }
    }
    out
}

fn parse_tier(s: &str) -> Option<HwTier> {
    match s {
        "base" => Some(HwTier::Base),
        "expanded" => Some(HwTier::Expanded),
        _ => None,
    }
}

/// A project scene: its file (relative to the root) and contents.
struct SceneFile {
    file: String,
    scene: Scene,
}

/// `scenes/*.scene.ron`, sorted.
fn scenes(root: &Path) -> Result<Vec<SceneFile>, String> {
    let mut files: Vec<PathBuf> = std::fs::read_dir(root.join("scenes"))
        .map(|d| {
            d.flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with(".scene.ron"))
                .collect()
        })
        .unwrap_or_default();
    files.sort();
    files
        .into_iter()
        .map(|p| {
            let scene = Scene::load(&p)?;
            let file = p
                .strip_prefix(root)
                .unwrap_or(&p)
                .to_string_lossy()
                .replace('\\', "/");
            Ok(SceneFile { file, scene })
        })
        .collect()
}

/// A group's bundle from the last bake on `platform`: (bytes once loaded,
/// bytes on the medium — the bundle and the files it names).
fn on_disk(root: &Path, platform: Platform, group: &str) -> Option<(u64, u64)> {
    let bundle = match platform {
        Platform::Pc => root.join(format!("target/assets/pc/groups/{group}.tbnd")),
        Platform::N64 => root.join(format!("target/n64/filesystem/groups/{group}.tbnd")),
        Platform::N3ds => root.join(format!("target/3ds/romfs/groups/{group}.tbnd")),
        Platform::Web => std::fs::read_dir(root.join("target/web/assets"))
            .ok()?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.to_string_lossy().ends_with(&format!(".{group}.tbnd")))?,
    };
    let bytes = std::fs::read(&bundle).ok()?;
    let b = trino_core::bundle::Bundle::parse(&bytes).ok()?;
    let fsdir = bundle.parent()?.parent()?;
    let mut raw = 0;
    let mut stored = bytes.len() as u64;
    for e in b.entries() {
        raw += e.raw as u64;
        if let Some(path) = b.path(&e) {
            let file = path.split('\t').next().unwrap_or(path);
            stored += std::fs::metadata(fsdir.join(file)).map_or(0, |m| m.len());
        }
    }
    Some((raw, stored))
}

fn report(root: &Path, opts: &[&str], json: bool) -> Result<bool, String> {
    let mut targets = consoles();
    let mut it = opts.iter();
    while let Some(a) = it.next() {
        match *a {
            "--platform" => {
                let p = it
                    .next()
                    .and_then(|p| Platform::parse(p))
                    .ok_or("--platform needs n64, 3ds, pc or web")?;
                targets = [HwTier::Base, HwTier::Expanded]
                    .into_iter()
                    .filter(|t| matches!(p, Platform::N64 | Platform::N3ds) || *t == HwTier::Base)
                    .map(|t| (p, t))
                    .collect();
            }
            "--tier" => {
                let t = it
                    .next()
                    .and_then(|t| parse_tier(t))
                    .ok_or("--tier needs base or expanded")?;
                targets.retain(|(_, tier)| *tier == t);
                if targets.is_empty() {
                    targets = consoles().into_iter().filter(|(_, x)| *x == t).collect();
                }
            }
            other => return Err(format!("unknown argument `{other}`\n{USAGE}")),
        }
    }
    let assets = root.join("assets");
    let scenes = scenes(root)?;
    let extra: Vec<(String, Vec<String>)> = scenes
        .iter()
        .map(|s| {
            (
                format!("scene {}", s.scene.name),
                groups::scene_groups(&s.scene),
            )
        })
        .collect();
    let mut ok = true;
    let mut reports = Vec::new();
    for (platform, tier) in targets {
        let plan = memory::plan(&assets, platform, tier)?;
        let r = PlanReport::new(root, plan, &extra);
        ok &= r.fits();
        reports.push(r);
    }
    if json {
        let list: Vec<serde_json::Value> = reports.iter().map(PlanReport::json).collect();
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({ "ok": ok, "platforms": list }))
                .unwrap()
        );
    } else {
        for r in &reports {
            r.print();
        }
    }
    Ok(ok)
}

struct GroupRow {
    name: String,
    assets: usize,
    bytes: u64,
    on_disk: Option<(u64, u64)>,
}

struct PlanReport {
    plan: MemoryPlan,
    groups: Vec<GroupRow>,
    /// (name, resident bytes).
    scenarios: Vec<(String, u64)>,
}

impl PlanReport {
    fn new(root: &Path, plan: MemoryPlan, extra: &[(String, Vec<String>)]) -> Self {
        let groups = plan
            .groups
            .iter()
            .map(|g| GroupRow {
                name: g.clone(),
                assets: plan
                    .assets
                    .iter()
                    .filter(|a| a.groups.iter().any(|x| x == g))
                    .count(),
                bytes: plan.group_bytes(g),
                on_disk: (g != trino_asset_pipeline::groups::BOOT)
                    .then(|| on_disk(root, plan.platform, g))
                    .flatten(),
            })
            .collect();
        PlanReport {
            scenarios: plan.scenarios(extra),
            plan,
            groups,
        }
    }

    fn fits(&self) -> bool {
        !self.plan.limited() || self.scenarios.iter().all(|(_, b)| *b <= self.plan.budget())
    }

    fn label(&self) -> String {
        format!(
            "{} {}",
            memory::platform_label(self.plan.platform),
            self.plan.tier.key()
        )
    }

    fn print(&self) {
        let p = &self.plan;
        if p.limited() {
            println!(
                "[{}] {} RAM - {} engine reserve - {} game = {} for assets",
                self.label(),
                show_bytes(p.ram),
                show_bytes(p.reserve),
                show_bytes(p.game),
                show_bytes(p.budget())
            );
        } else {
            println!("[{}] no RAM ceiling", self.label());
        }
        println!(
            "  {:<24} {:>6} {:>10} {:>16}",
            "group", "assets", "in RAM", "baked (stored)"
        );
        for g in &self.groups {
            let disk = g
                .on_disk
                .map_or_else(|| "-".to_string(), |(_, stored)| show_bytes(stored));
            println!(
                "  {:<24} {:>6} {:>10} {:>16}",
                g.name,
                g.assets,
                show_bytes(g.bytes),
                disk
            );
        }
        println!("  {:<31} {:>10}  of the budget", "scenario", "in RAM");
        for (name, bytes) in &self.scenarios {
            let verdict = if !p.limited() {
                String::new()
            } else if *bytes <= p.budget() {
                format!("{:>3}%", bytes * 100 / p.budget().max(1))
            } else {
                format!("OVER by {}", show_bytes(bytes - p.budget()))
            };
            println!("  {:<31} {:>10}  {verdict}", name, show_bytes(*bytes));
        }
    }

    fn json(&self) -> serde_json::Value {
        let p = &self.plan;
        serde_json::json!({
            "platform": p.platform.key(),
            "tier": p.tier.key(),
            "ram": p.limited().then_some(p.ram),
            "reserve": p.reserve,
            "game": p.game,
            "budget": p.limited().then_some(p.budget()),
            "groups": self.groups.iter().map(|g| serde_json::json!({
                "name": g.name,
                "assets": g.assets,
                "bytes": g.bytes,
                "baked_raw": g.on_disk.map(|d| d.0),
                "baked_stored": g.on_disk.map(|d| d.1),
            })).collect::<Vec<_>>(),
            "scenarios": self.scenarios.iter().map(|(name, bytes)| serde_json::json!({
                "name": name,
                "bytes": bytes,
                "fits": !p.limited() || *bytes <= p.budget(),
            })).collect::<Vec<_>>(),
        })
    }
}

/// One scene's problems: its groups against the manifest
/// (`trino_asset_pipeline::groups::scene_problems`), then every console
/// tier's RAM with all of them loaded.
fn scene_problems(
    file: &str,
    scene: &Scene,
    lib: &Library,
    manifest: &Manifest,
    plans: &[MemoryPlan],
) -> Vec<String> {
    let mut out: Vec<String> = groups::scene_problems(scene, lib, manifest)
        .into_iter()
        .map(|p| format!("{file}: {p}"))
        .collect();
    let all = groups::scene_groups(scene);
    let refs: Vec<&str> = all.iter().map(String::as_str).collect();
    for plan in plans {
        if let Err(e) = plan.check(&refs) {
            out.push(format!("{file}: {e}"));
        }
    }
    out
}

fn check(root: &Path, json: bool) -> Result<bool, String> {
    let manifest = Manifest::load(&root.join("assets/manifest.toml"))?;
    let lib = crate::scene::load_library(root);
    let scenes = scenes(root)?;
    let plans: Vec<MemoryPlan> = consoles()
        .into_iter()
        .map(|(p, t)| memory::plan(&root.join("assets"), p, t))
        .collect::<Result<_, _>>()?;
    let mut problems = Vec::new();
    let mut rows = Vec::new();
    for s in &scenes {
        let p = scene_problems(&s.file, &s.scene, &lib, &manifest, &plans);
        rows.push(serde_json::json!({
            "file": s.file,
            "requires": s.scene.requires,
            "cell_groups": s.scene.cell_groups().into_iter().map(|(c, g)| serde_json::json!({ "cell": c, "groups": g })).collect::<Vec<_>>(),
            "problems": p,
        }));
        if !json {
            if p.is_empty() {
                println!("ok  {} (requires {:?})", s.file, s.scene.requires);
            } else {
                for e in &p {
                    eprintln!("error: {e}");
                }
            }
        }
        problems.extend(p);
    }
    let ok = problems.is_empty();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&serde_json::json!({
                "ok": ok,
                "groups": manifest.group_names(),
                "scenes": rows,
            }))
            .unwrap()
        );
    } else {
        println!(
            "{} scene(s), {} group(s), {} problem(s)",
            scenes.len(),
            manifest.group_names().len(),
            problems.len()
        );
    }
    Ok(ok)
}

fn group(root: &Path, logical: &str, groups: &str, json: bool) -> Result<bool, String> {
    let path = root.join("assets/manifest.toml");
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let list: Vec<String> = groups
        .split(',')
        .map(|g| g.trim().to_string())
        .filter(|g| !g.is_empty())
        .collect();
    let edited = trino_asset_pipeline::manifest::edit::set_groups(&text, logical, &list)?;
    // What the bake will read: an edit that breaks the manifest is not
    // written (checked beside it, then moved over it).
    let tmp = path.with_extension("toml.edit");
    std::fs::write(&tmp, &edited).map_err(|e| format!("{}: {e}", tmp.display()))?;
    let manifest = match Manifest::load(&tmp) {
        Ok(m) => m,
        Err(e) => {
            let _ = std::fs::remove_file(&tmp);
            return Err(e);
        }
    };
    std::fs::rename(&tmp, &path).map_err(|e| format!("{}: {e}", path.display()))?;
    let now = manifest.groups_of(logical);
    if json {
        println!(
            "{}",
            serde_json::json!({ "asset": logical, "groups": now, "declared": manifest.group_names() })
        );
    } else {
        println!("{logical}: groups {now:?}");
    }
    Ok(true)
}
