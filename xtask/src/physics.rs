//! `cargo xtask physics <check|query|layers>` (issue #66): a scene's
//! collision as an agent sees it, without the editor.
//!
//! - `physics check [scene.ron ...] [--json]` — every scene (default: all
//!   of `scenes/`): its solids, one-way platforms, ramps, mesh colliders,
//!   triggers and characters; problems (overlapping static solids, mesh
//!   colliders with no baked `collision`, degenerate collision triangles,
//!   unnamed triggers, unknown layers, ramps too steep for every character)
//!   and the collision-triangle budget per platform (`Caps::
//!   max_collision_tris`). Non-zero exit on an error.
//! - `physics query <scene.ron> --ray x,y,z dx,dy,dz [--max m]`,
//!   `--cast sphere:r|capsule:r,h|box:x,y,z x,y,z x,y,z`,
//!   `--overlap x,y,z hx,hy,hz`, `--drop x,y,z [--character r,h,slope,step]`
//!   `[--mask bits] [--json]` — what the engine's queries answer (the same
//!   `StaticWorld` grid and mesh instances a game builds).
//! - `physics layers [--json]`, `layers add <name>`, `layers remove <name>`,
//!   `layers set <a> <b> on|off` — `assets/physics.ron`, as the editor's
//!   Project › Physics matrix edits it.

use std::path::{Path, PathBuf};
use std::process::ExitCode;

use serde_json::json;
use trino_core::Vec3;
use trino_core::physics::{
    CastShape, Character, CharacterController, LAYER_ALL, MeshCollider, MeshInstance, Shape, Solid,
    StaticWorld, Surface, SurfaceKind, V3, World,
};
use trino_scene::physics::PhysicsSettings;
use trino_scene::{Scene, Target, resolve_for};

const USAGE: &str = "xtask: usage:\n  cargo xtask physics check [scene.ron ...] [--json]\n  \
     cargo xtask physics query <scene.ron> (--ray x,y,z dx,dy,dz [--max m] | \
     --cast sphere:r|capsule:r,h|box:hx,hy,hz x,y,z x,y,z | --overlap x,y,z hx,hy,hz | \
     --drop x,y,z [--character r,h,slope,step]) [--mask bits] [--json]\n  \
     cargo xtask physics layers [--json] | layers add <name> | layers remove <name> | \
     layers set <a> <b> on|off";

pub fn run(root: &Path, args: &[&str]) -> ExitCode {
    match args {
        ["check", rest @ ..] => check(root, rest),
        ["query", rest @ ..] => query(root, rest),
        ["layers", rest @ ..] => layers(root, rest),
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

/// A scene's collision, resolved like the bake does (PC: variants are a
/// look, colliders stay the PC ones).
pub struct Level {
    pub solids: Vec<Solid>,
    /// Entity name of each solid.
    pub solid_names: Vec<String>,
    pub triggers: Vec<(String, Solid)>,
    /// (entity, model, its matrix, the baked TCOL blob if any).
    pub meshes: Vec<(String, String, trino_core::Mat34, Option<Vec<u8>>)>,
    pub characters: Vec<(String, Vec3, Character)>,
    pub warnings: Vec<String>,
}

impl Level {
    pub fn load(root: &Path, scene: &Scene) -> Result<Level, String> {
        let lib = crate::scene::load_library(root);
        let r = resolve_for(scene, &lib, 0.0, Target::Pc);
        let manifest = trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml"))?;
        let baked = trino_asset_pipeline::collision::bake_all(
            &root.join("assets"),
            &manifest,
            trino_asset_pipeline::Platform::Pc,
        )
        .unwrap_or_default();
        let mut level = Level {
            solids: Vec::new(),
            solid_names: Vec::new(),
            triggers: Vec::new(),
            meshes: Vec::new(),
            characters: Vec::new(),
            warnings: r.warnings.clone(),
        };
        for item in &r.items {
            if let Some(ch) = item.character {
                let p = item.matrix;
                level.characters.push((
                    item.name.clone(),
                    Vec3::new(p.m[0][3], p.m[1][3], p.m[2][3]),
                    ch,
                ));
            }
            let Some(c) = item.collider else {
                continue;
            };
            if let Some(t) = c.world_trigger(&item.matrix) {
                level.triggers.push((item.name.clone(), t));
            } else if let Some(s) = c.world_solid(&item.matrix) {
                level.solids.push(s);
                level.solid_names.push(item.name.clone());
            } else if let Shape::Mesh { mesh, .. } = c.shape {
                let model = scene_mesh_model(scene, &item.name).unwrap_or_default();
                let blob = baked
                    .iter()
                    .find(|m| trino_core::asset_id(&m.logical) == mesh)
                    .map(|m| m.bytes.clone());
                level
                    .meshes
                    .push((item.name.clone(), model, item.matrix, blob));
            }
        }
        Ok(level)
    }

    /// The meshes with a baked blob, placed.
    pub fn instances(&self) -> Vec<(usize, MeshInstance<'_>)> {
        self.meshes
            .iter()
            .enumerate()
            .filter_map(|(i, (_, _, m, blob))| {
                let mesh = MeshCollider::parse(blob.as_ref()?).ok()?;
                Some((i, MeshInstance::new(mesh, *m)))
            })
            .collect()
    }
}

/// The model a `Mesh("models/x")` collider names, by entity path.
fn scene_mesh_model(scene: &Scene, name: &str) -> Option<String> {
    let mut found = None;
    scene.walk(|path, e| {
        if found.is_none()
            && let Some(c) = &e.collider
            && let trino_scene::ShapeDef::Mesh(m) = &c.shape
        {
            let full = scene.draw_name(path).unwrap_or_default();
            if full == name || name.ends_with(&format!("/{}", e.name)) || e.name == name {
                found = Some(m.clone());
            }
        }
    });
    found
}

fn scenes(root: &Path, files: &[&str]) -> Vec<PathBuf> {
    if files.is_empty() {
        let mut v: Vec<PathBuf> = std::fs::read_dir(root.join("scenes"))
            .map(|d| {
                d.flatten()
                    .map(|e| e.path())
                    .filter(|p| p.to_string_lossy().ends_with(".scene.ron"))
                    .collect()
            })
            .unwrap_or_default();
        v.sort();
        v
    } else {
        files.iter().map(|f| root.join(f)).collect()
    }
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
enum Severity {
    Info,
    Warning,
    Error,
}

impl Severity {
    fn label(self) -> &'static str {
        match self {
            Severity::Info => "info",
            Severity::Warning => "warning",
            Severity::Error => "error",
        }
    }
}

/// The overlap volume of two boxes (0 when apart or touching).
fn overlap_volume(a: &trino_core::Aabb, b: &trino_core::Aabb) -> f32 {
    let d = |lo: f32, hi: f32| (hi - lo).max(0.0);
    d(a.min.x.max(b.min.x), a.max.x.min(b.max.x))
        * d(a.min.y.max(b.min.y), a.max.y.min(b.max.y))
        * d(a.min.z.max(b.min.z), a.max.z.min(b.max.z))
}

fn check(root: &Path, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let files: Vec<&str> = args
        .iter()
        .copied()
        .filter(|a| !a.starts_with("--"))
        .collect();
    let mut report = Vec::new();
    let mut failed = false;
    let layer_problems = match PhysicsSettings::load(root) {
        Ok(_) => Vec::new(),
        Err(e) => vec![e],
    };
    for e in &layer_problems {
        failed = true;
        if !json {
            eprintln!("error: {e}");
        }
    }
    for path in scenes(root, &files) {
        let rel = path
            .strip_prefix(root)
            .unwrap_or(&path)
            .display()
            .to_string();
        let mut issues: Vec<(Severity, String)> = Vec::new();
        let scene = match Scene::load(&path) {
            Ok(s) => s,
            Err(e) => {
                failed = true;
                issues.push((Severity::Error, e));
                report.push(json!({ "scene": rel, "issues": issues_json(&issues) }));
                continue;
            }
        };
        let level = match Level::load(root, &scene) {
            Ok(l) => l,
            Err(e) => {
                failed = true;
                issues.push((Severity::Error, e));
                report.push(json!({ "scene": rel, "issues": issues_json(&issues) }));
                continue;
            }
        };
        for w in &level.warnings {
            let sev = if w.contains("collision layer") || w.contains("mesh collider") {
                Severity::Error
            } else {
                Severity::Warning
            };
            issues.push((sev, w.clone()));
        }
        // Overlapping static solids (a ramp into a deck, a box inside a
        // wall: usually a typo in a size).
        for i in 0..level.solids.len() {
            for j in i + 1..level.solids.len() {
                let (a, b) = (&level.solids[i], &level.solids[j]);
                if a.layer & b.layer == 0 {
                    continue;
                }
                let v = overlap_volume(&a.aabb, &b.aabb);
                if v > 1e-3 {
                    issues.push((
                        Severity::Warning,
                        format!(
                            "solids `{}` and `{}` overlap ({v:.3} m³)",
                            level.solid_names[i], level.solid_names[j]
                        ),
                    ));
                }
            }
        }
        for (name, model, _, blob) in &level.meshes {
            match blob {
                None => issues.push((
                    Severity::Error,
                    format!(
                        "{name}: mesh collider `{model}` has no baked collision (add \
                         `collision = \"visual\"`, `\"auto-hull\"` or a file to \
                         [models.{}] in the manifest)",
                        model.trim_start_matches("models/")
                    ),
                )),
                Some(b) => {
                    if let Err(e) = MeshCollider::parse(b) {
                        issues.push((Severity::Error, format!("{name}: bad TCOL: {e:?}")));
                    }
                }
            }
        }
        for (name, _) in &level.triggers {
            let leaf = name.rsplit('/').next().unwrap_or(name);
            if leaf.is_empty() || leaf.starts_with("Entity") {
                issues.push((
                    Severity::Error,
                    format!(
                        "trigger `{name}` has no name of its own: game code finds triggers \
                         by name or tag"
                    ),
                ));
            }
        }
        // Ramps nobody here can walk.
        for (i, s) in level.solids.iter().enumerate() {
            if let SurfaceKind::Ramp(_) = s.kind {
                let surf: Surface<f32> = s.into();
                let steepest_allowed = level
                    .characters
                    .iter()
                    .map(|c| c.2.max_slope_deg)
                    .fold(Character::DEFAULT.max_slope_deg, f32::max);
                let n = surf.normal();
                let deg = n.y.clamp(-1.0, 1.0).acos().to_degrees();
                if deg > steepest_allowed + 0.01 {
                    issues.push((
                        Severity::Info,
                        format!(
                            "ramp `{}` is {deg:.1}°: steeper than any character's limit \
                             ({steepest_allowed:.0}°), so it is a wall and a slide",
                            level.solid_names[i]
                        ),
                    ));
                }
            }
        }
        // Budgets.
        let tris: usize = level
            .meshes
            .iter()
            .filter_map(|m| m.3.as_ref())
            .filter_map(|b| MeshCollider::parse(b).ok())
            .map(|m| m.triangle_count())
            .sum();
        let mut budgets = Vec::new();
        for (target, caps) in [
            ("n64", trino_core::Caps::N64),
            ("3ds", trino_core::Caps::N3DS),
            ("pc", trino_core::Caps::PC),
            ("web", trino_core::Caps::WEB),
        ] {
            let over = tris as u32 > caps.max_collision_tris;
            if over {
                issues.push((
                    Severity::Error,
                    format!(
                        "{tris} collision triangles: over the {target} budget of {} \
                         (simplify the collision meshes or use auto-hull)",
                        caps.max_collision_tris
                    ),
                ));
            }
            budgets.push(json!({
                "platform": target,
                "collision_tris": tris,
                "max_collision_tris": caps.max_collision_tris,
                "fits": !over,
            }));
        }
        let grid = (!level.solids.is_empty())
            .then(|| StaticWorld::size(&level.solids, GRID_CELL))
            .flatten();
        let count =
            |k: fn(&SurfaceKind) -> bool| level.solids.iter().filter(|s| k(&s.kind)).count();
        let boxes = count(|k| *k == SurfaceKind::Solid);
        let one_way = count(|k| *k == SurfaceKind::OneWay);
        let ramps = count(|k| matches!(k, SurfaceKind::Ramp(_)));
        failed |= issues.iter().any(|(s, _)| *s == Severity::Error);
        if !json {
            println!(
                "{rel}: {boxes} solid(s), {one_way} one-way, {ramps} ramp(s), {} mesh collider(s) \
                 ({tris} triangles), {} trigger(s), {} character(s){}",
                level.meshes.len(),
                level.triggers.len(),
                level.characters.len(),
                grid.map_or(String::new(), |g| format!(
                    "; grid {} cells, {} refs ({} B)",
                    g.starts - 1,
                    g.refs,
                    (g.starts + g.refs) * 2
                ))
            );
            for (sev, msg) in &issues {
                println!("  {}: {msg}", sev.label());
            }
        }
        report.push(json!({
            "scene": rel,
            "solids": boxes,
            "one_way": one_way,
            "ramps": ramps,
            "meshes": level.meshes.iter().map(|(n, m, _, b)| json!({
                "entity": n,
                "model": m,
                "triangles": b.as_ref().and_then(|b| MeshCollider::parse(b).ok()).map(|m| m.triangle_count()),
            })).collect::<Vec<_>>(),
            "triggers": level.triggers.iter().map(|(n, _)| n.clone()).collect::<Vec<_>>(),
            "characters": level.characters.iter().map(|(n, at, c)| json!({
                "entity": n,
                "at": [at.x, at.y, at.z],
                "radius": c.radius,
                "height": c.height,
                "max_slope_deg": c.max_slope_deg,
                "step_height": c.step_height,
            })).collect::<Vec<_>>(),
            "grid": grid.map(|g| json!({ "cells": g.starts - 1, "refs": g.refs, "bytes": (g.starts + g.refs) * 2 })),
            "budget": budgets,
            "issues": issues_json(&issues),
        }));
    }
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "ok": !failed,
                "layers": layer_problems,
                "scenes": report,
            }))
            .unwrap()
        );
    }
    if failed {
        ExitCode::FAILURE
    } else {
        ExitCode::SUCCESS
    }
}

fn issues_json(issues: &[(Severity, String)]) -> Vec<serde_json::Value> {
    issues
        .iter()
        .map(|(s, m)| json!({ "level": s.label(), "message": m }))
        .collect()
}

/// The grid cell the tools build levels with (games pick their own).
pub const GRID_CELL: f32 = 8.0;

fn vec3(s: &str) -> Result<Vec3, String> {
    let v: Vec<f32> = s
        .split(',')
        .map(|p| p.trim().parse::<f32>().map_err(|e| format!("`{s}`: {e}")))
        .collect::<Result<_, _>>()?;
    match v.as_slice() {
        [x, y, z] => Ok(Vec3::new(*x, *y, *z)),
        _ => Err(format!("`{s}`: expected x,y,z")),
    }
}

fn cast_shape(s: &str) -> Result<CastShape, String> {
    let (kind, rest) = s
        .split_once(':')
        .ok_or(format!("`{s}`: expected kind:params"))?;
    let nums: Vec<f32> = rest
        .split(',')
        .map(|p| p.trim().parse::<f32>().map_err(|e| format!("`{s}`: {e}")))
        .collect::<Result<_, _>>()?;
    match (kind, nums.as_slice()) {
        ("sphere", [r]) => Ok(CastShape::Sphere { radius: *r }),
        ("capsule", [r, h]) => Ok(CastShape::Capsule {
            radius: *r,
            height: *h,
        }),
        ("box", [x, y, z]) => Ok(CastShape::Box {
            half: Vec3::new(*x, *y, *z),
        }),
        _ => Err(format!(
            "`{s}`: expected sphere:r, capsule:r,h or box:hx,hy,hz"
        )),
    }
}

fn query(root: &Path, args: &[&str]) -> ExitCode {
    match query_inner(root, args) {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}\n{USAGE}");
            ExitCode::FAILURE
        }
    }
}

fn query_inner(root: &Path, args: &[&str]) -> Result<(), String> {
    let mut json = false;
    let mut file = None;
    let mut ray = None;
    let mut cast = None;
    let mut overlap = None;
    let mut drop = None;
    let mut max = 100.0f32;
    let mut mask = LAYER_ALL;
    let mut character = Character::DEFAULT;
    let mut it = args.iter();
    while let Some(a) = it.next() {
        let mut next = || it.next().copied().ok_or(format!("`{a}` needs a value"));
        match *a {
            "--json" => json = true,
            "--ray" => ray = Some((vec3(next()?)?, vec3(next()?)?)),
            "--cast" => cast = Some((cast_shape(next()?)?, vec3(next()?)?, vec3(next()?)?)),
            "--overlap" => overlap = Some((vec3(next()?)?, vec3(next()?)?)),
            "--drop" => drop = Some(vec3(next()?)?),
            "--max" => max = next()?.parse().map_err(|e| format!("--max: {e}"))?,
            "--mask" => {
                let v = next()?;
                mask = if let Some(h) = v.strip_prefix("0x") {
                    u16::from_str_radix(h, 16)
                } else {
                    v.parse()
                }
                .map_err(|e| format!("--mask: {e}"))?;
            }
            "--character" => {
                let v: Vec<f32> = next()?
                    .split(',')
                    .map(|p| {
                        p.trim()
                            .parse::<f32>()
                            .map_err(|e| format!("--character: {e}"))
                    })
                    .collect::<Result<_, _>>()?;
                let [r, h, slope, step] = v.as_slice() else {
                    return Err("--character needs radius,height,max_slope_deg,step_height".into());
                };
                character = Character {
                    radius: *r,
                    height: *h,
                    max_slope_deg: *slope,
                    step_height: *step,
                };
            }
            other if file.is_none() && !other.starts_with("--") => file = Some(other),
            other => return Err(format!("unexpected `{other}`")),
        }
    }
    let file = file.ok_or("which scene?")?;
    let scene = Scene::load(&root.join(file))?;
    let level = Level::load(root, &scene)?;
    let size = StaticWorld::size(&level.solids, GRID_CELL);
    let (mut starts, mut refs) = (
        vec![0u16; size.map_or(1, |s| s.starts)],
        vec![0u16; size.map_or(1, |s| s.refs)],
    );
    let grid = StaticWorld::build(&level.solids, GRID_CELL, &mut starts, &mut refs);
    let instances = level.instances();
    let name_of = |i: usize| level.solid_names.get(i).cloned().unwrap_or_default();
    let kind_of = |i: usize| match level.solids.get(i).map(|s| s.kind) {
        Some(SurfaceKind::Solid) => "solid",
        Some(SurfaceKind::OneWay) => "one_way",
        Some(SurfaceKind::Ramp(_)) => "ramp",
        None => "",
    };
    let out = if let Some((origin, dir)) = ray {
        let dir = dir.normalized();
        let solid = grid.and_then(|g| g.raycast(origin, dir, max, mask));
        let mut best = solid.map(|h| {
            json!({
                "t": h.t, "entity": name_of(h.index), "kind": kind_of(h.index),
                "normal": [h.normal.x, h.normal.y, h.normal.z],
            })
        });
        let mut best_t = solid.map_or(max, |h| h.t);
        for (mi, inst) in &instances {
            if let Some(h) = inst.raycast(origin, dir, best_t)
                && h.t < best_t
            {
                best_t = h.t;
                best = Some(json!({
                    "t": h.t, "entity": level.meshes[*mi].0, "kind": "mesh", "triangle": h.tri,
                    "normal": [h.normal.x, h.normal.y, h.normal.z],
                }));
            }
        }
        if let Some(b) = best.as_mut() {
            let p = origin + dir * best_t;
            b["point"] = json!([p.x, p.y, p.z]);
        }
        json!({ "query": "ray", "origin": [origin.x, origin.y, origin.z], "dir": [dir.x, dir.y, dir.z], "hit": best })
    } else if let Some((shape, from, to)) = cast {
        let hit = grid.and_then(|g| g.shape_cast(shape, from, to, mask));
        json!({
            "query": "cast", "shape": format!("{shape:?}"),
            "hit": hit.map(|h| json!({
                "t": h.t, "fraction": h.fraction, "entity": name_of(h.index), "kind": kind_of(h.index),
                "normal": [h.normal.x, h.normal.y, h.normal.z], "inside": h.inside,
            })),
            "note": "shape casts test boxes, one-way boxes and ramps as their box; rays test meshes too",
        })
    } else if let Some((c, h)) = overlap {
        let q = trino_core::Aabb::from_center_half(c, h);
        let mut ids = vec![0u16; level.solids.len().max(1)];
        let n = grid.map_or(0, |g| g.overlap_aabb(&q, mask, &mut ids));
        let triggers: Vec<String> = level
            .triggers
            .iter()
            .filter(|(_, t)| t.layer & mask != 0 && t.aabb.overlaps(&q))
            .map(|(n, _)| n.clone())
            .collect();
        json!({
            "query": "overlap",
            "solids": ids[..n].iter().map(|&i| name_of(i as usize)).collect::<Vec<_>>(),
            "triggers": triggers,
        })
    } else if let Some(at) = drop {
        // A character dropped at `at` (its feet) settles for up to 4 s.
        let cfg = character.config(25.0, 30.0);
        let mut c = CharacterController::new(cfg, V3::new(at.x, at.y + cfg.half.y, at.z));
        c.cfg.mask = mask;
        let world = grid.map(|g| World {
            statics: g,
            dynamics: &[],
        });
        let meshes: Vec<&dyn trino_core::physics::MeshQuery<f32>> = instances
            .iter()
            .map(|(_, m)| m as &dyn trino_core::physics::MeshQuery<f32>)
            .collect();
        let mut rep = None;
        for _ in 0..240 {
            let r = match &world {
                Some(w) => c.step_world(1.0 / 60.0, w, &meshes),
                None => c.step(1.0 / 60.0, &[] as &[Surface<f32>], &meshes),
            };
            rep = Some(r);
            if c.on_ground && c.vel.y == 0.0 && r.on_ground {
                break;
            }
        }
        let rep = rep.unwrap();
        let feet = Vec3::new(c.pos.x, c.pos.y - cfg.half.y, c.pos.z);
        json!({
            "query": "drop",
            "feet": [feet.x, feet.y, feet.z],
            "on_ground": c.on_ground,
            "sliding": rep.sliding,
            "ground": match (rep.standing_on, rep.mesh_ground) {
                (Some(i), _) => json!({ "entity": name_of(i), "kind": kind_of(i) }),
                (None, Some((m, t))) => json!({ "entity": level.meshes[instances[m].0].0, "kind": "mesh", "triangle": t }),
                _ => serde_json::Value::Null,
            },
            "normal": [c.ground_normal.x, c.ground_normal.y, c.ground_normal.z],
        })
    } else {
        return Err("give --ray, --cast, --overlap or --drop".into());
    };
    if json {
        println!("{}", serde_json::to_string_pretty(&out).unwrap());
    } else {
        println!("{}", human(&out));
    }
    Ok(())
}

fn human(v: &serde_json::Value) -> String {
    let q = v["query"].as_str().unwrap_or("");
    match q {
        "ray" | "cast" => match &v["hit"] {
            serde_json::Value::Null => format!("{q}: no hit"),
            h => format!(
                "{q}: hit `{}` ({}) at t = {:.3}{}, normal {}",
                h["entity"].as_str().unwrap_or(""),
                h["kind"].as_str().unwrap_or(""),
                h["t"].as_f64().unwrap_or(0.0),
                h.get("point")
                    .map(|p| format!(", point {p}"))
                    .unwrap_or_default(),
                h["normal"]
            ),
        },
        "overlap" => format!("overlap: solids {} triggers {}", v["solids"], v["triggers"]),
        "drop" => format!(
            "drop: feet at {}, {}, on {}",
            v["feet"],
            if v["on_ground"].as_bool() == Some(true) {
                "grounded"
            } else if v["sliding"].as_bool() == Some(true) {
                "sliding"
            } else {
                "in the air"
            },
            v["ground"]
        ),
        _ => v.to_string(),
    }
}

fn layers(root: &Path, args: &[&str]) -> ExitCode {
    let json = args.contains(&"--json");
    let args: Vec<&str> = args.iter().copied().filter(|a| *a != "--json").collect();
    let mut s = match PhysicsSettings::load(root) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("xtask: {e}");
            return ExitCode::FAILURE;
        }
    };
    let changed = match args.as_slice() {
        [] => false,
        ["add", name] => {
            if s.layers.iter().any(|l| l == name) {
                eprintln!("xtask: layer `{name}` exists");
                return ExitCode::FAILURE;
            }
            s.layers.push(name.to_string());
            true
        }
        ["remove", name] => {
            let Some(i) = s.layers.iter().position(|l| l == name) else {
                eprintln!("xtask: no layer `{name}`");
                return ExitCode::FAILURE;
            };
            s.layers.remove(i);
            s.ignore.retain(|(a, b)| a != name && b != name);
            true
        }
        ["set", a, b, on @ ("on" | "off")] => {
            for n in [a, b] {
                if !s.layers.iter().any(|l| l == n) {
                    eprintln!("xtask: no layer `{n}`");
                    return ExitCode::FAILURE;
                }
            }
            s.set_collide(a, b, *on == "on");
            true
        }
        _ => {
            eprintln!("{USAGE}");
            return ExitCode::FAILURE;
        }
    };
    let problems = s.problems();
    if !problems.is_empty() {
        eprintln!("xtask: {}", problems.join("; "));
        return ExitCode::FAILURE;
    }
    if changed && let Err(e) = s.save(root) {
        eprintln!("xtask: {e}");
        return ExitCode::FAILURE;
    }
    let masks = s.masks();
    if json {
        println!(
            "{}",
            serde_json::to_string_pretty(&json!({
                "file": trino_scene::physics::PHYSICS_FILE,
                "layers": s.layers.iter().zip(&masks).enumerate().map(|(i, (l, m))| json!({
                    "name": l, "bit": 1u32 << i, "mask": m,
                    "collides_with": s.layers.iter().filter(|o| s.collide(l, o)).collect::<Vec<_>>(),
                })).collect::<Vec<_>>(),
                "ignore": s.ignore,
            }))
            .unwrap()
        );
    } else {
        for (i, (l, m)) in s.layers.iter().zip(&masks).enumerate() {
            let with: Vec<&str> = s
                .layers
                .iter()
                .filter(|o| s.collide(l, o))
                .map(String::as_str)
                .collect();
            println!(
                "{i:>2} {l:<12} mask {m:#06x}  collides with {}",
                with.join(", ")
            );
        }
    }
    ExitCode::SUCCESS
}
