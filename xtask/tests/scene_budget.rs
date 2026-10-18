//! The per-platform frame estimate (`trino_scene::budget`, the editor's
//! Game-view HUD and `cargo xtask scene budget`) against the real thing:
//! its T&L work equals the PC renderer's counters under the N64 profile for
//! the same scene and camera; a mesh behind the camera shows up as sent for
//! nothing; the N64's RSP split names each rule; variants, LOD levels and
//! impostors change it the way they change the game.

use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;

use trino_core::{Camera3, LodPick, Material, Renderer, Vec2, Vec3};
use trino_platform_pc::{PcRenderer, SimProfile};
use trino_scene::budget::{self, FrameBudget, RenderPath};
use trino_scene::{Entity, Library, ModelComponent, Scene, Target, resolve_for};

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Every model of the repo, baked for the PC, and the library.
fn assets() -> (HashMap<u32, Vec<u8>>, Library) {
    let assets = root().join("assets");
    let manifest = trino_asset_pipeline::Manifest::load(&assets.join("manifest.toml")).unwrap();
    let meshes = manifest
        .models
        .iter()
        .map(|(name, d)| {
            let src = trino_asset_pipeline::resolve_source(
                &assets,
                trino_asset_pipeline::Platform::Pc,
                &d.file,
            )
            .unwrap();
            let blob = trino_asset_pipeline::bake_model(&src, &d.options()).unwrap();
            (trino_core::asset_id(&format!("models/{name}")), blob)
        })
        .collect();
    let mut lib = Library::load(&root());
    lib.model_bounds = trino_asset_pipeline::manifest_model_bounds(&assets);
    lib.impostors = trino_asset_pipeline::impostor::baked(&assets);
    (meshes, lib)
}

fn estimate(
    scene: &Scene,
    lib: &Library,
    meshes: &HashMap<u32, Vec<u8>>,
    t: Target,
) -> FrameBudget {
    let env = &scene.environment;
    budget::estimate(
        &resolve_for(scene, lib, 0.0, t),
        &|id| meshes.get(&id.0).map(Vec::as_slice),
        &env.core_camera(),
        &env.core_light(),
        budget::screen_of(t, Vec2::new(640.0, 480.0)),
        t,
    )
}

#[test]
fn the_estimate_matches_the_pc_renderers_counters_on_the_n64_profile() {
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(SimProfile::N64)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (meshes, lib) = assets();
    for (id, blob) in &meshes {
        renderer.upload_mesh(trino_core::ModelId(*id), blob.clone());
    }
    for file in ["scenes/showcase.scene.ron", "scenes/interior.scene.ron"] {
        let scene = Scene::load(&root().join(file)).unwrap();
        let b = estimate(&scene, &lib, &meshes, Target::N64);
        // The renderer draws what the game would, from the same camera.
        let env = &scene.environment;
        let (camera, light) = (env.core_camera(), env.core_light());
        let resolved = resolve_for(&scene, &lib, 0.0, Target::N64);
        let visible = resolved.visible_cells(&camera, Vec2::new(320.0, 240.0));
        renderer.begin_frame(trino_core::Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&light);
        for item in &resolved.items {
            if !trino_core::vis::in_view(item.cell, visible) {
                continue;
            }
            match item.lod_from(camera.eye) {
                LodPick::Model(id) => {
                    renderer.draw_model_matrix(id, &item.matrix, Material::VertexLit, &item.params)
                }
                LodPick::Impostor(imp) => {
                    imp.draw(&mut renderer, &item.matrix, camera.eye, item.params.tint)
                }
                LodPick::None => {}
            }
        }
        renderer.end_frame();
        let stats = renderer.stats();
        assert_eq!(stats.tnl, b.tnl, "{file}: the same T&L work");
        assert_eq!(stats.models, b.draws, "{file}: the same draws");
        assert!(b.draws > 0, "{file}");
        // The N64 time is the slower of its CPU and RSP halves.
        assert_eq!(b.n64_frame_us(), b.n64_cpu_us().max(b.n64_rsp_us()));
        assert_eq!(b.rsp_draws + b.cpu_draws, b.draws);
        let off: u32 = b.off_rsp.iter().map(|(_, n)| n).sum();
        assert_eq!(off, b.cpu_draws, "{file}: every CPU draw has its reason");
    }
}

fn rock_at(name: &str, pos: [f32; 3]) -> Entity {
    Entity::new(name)
        .at(pos)
        .with_model(ModelComponent::new("models/rock"))
}

#[test]
fn a_mesh_behind_the_camera_is_sent_for_nothing() {
    let (meshes, lib) = assets();
    let mut scene = Scene::new("budget");
    scene.environment.camera.eye = [0.0, 2.0, -8.0];
    scene.environment.camera.target = [0.0, 0.5, 0.0];
    scene.entities.push(rock_at("in-view", [0.0, 0.0, 0.0]));
    scene.entities.push(rock_at("behind", [0.0, 0.0, -20.0]));
    for t in [Target::N64, Target::N3ds, Target::Pc] {
        let b = estimate(&scene, &lib, &meshes, t);
        assert_eq!(b.draws, 2, "{t:?}");
        assert_eq!(b.wasted, ["behind"], "{t:?}");
        assert_eq!(b.tnl.culled, 1);
    }
    // On the N64 it is also off the RSP, as culled work.
    let n64 = estimate(&scene, &lib, &meshes, Target::N64);
    assert!(
        n64.off_rsp
            .iter()
            .any(|(r, _)| *r == trino_core::RspReject::Offscreen),
        "{:?}",
        n64.off_rsp
    );
}

#[test]
fn the_rsp_split_names_each_rule_and_follows_the_scene() {
    let (meshes, lib) = assets();
    let mut scene = Scene::new("rsp");
    scene.environment.camera.eye = [0.0, 2.0, -8.0];
    scene.environment.camera.target = [0.0, 0.5, 0.0];
    scene.environment.light.color = [255, 255, 255];
    scene.environment.light.ambient_color = [255, 255, 255];
    scene.entities.push(rock_at("plain", [0.0, 0.0, 0.0]));
    let mut flat = rock_at("flat", [2.0, 0.0, 0.0]);
    flat.model.as_mut().unwrap().material = Some("materials/stone".into());
    scene.entities.push(flat);
    let mut glass = rock_at("glass", [-2.0, 0.0, 0.0]);
    glass.model.as_mut().unwrap().tint = Some([255, 255, 255, 120]);
    scene.entities.push(glass);
    let b = estimate(&scene, &lib, &meshes, Target::N64);
    assert_eq!(b.draws, 3);
    assert!(
        b.rsp_draws >= 1,
        "the plain rock takes the fast path: {b:?}"
    );
    assert!(
        b.off_rsp
            .iter()
            .any(|(r, _)| *r == trino_core::RspReject::Translucent)
    );
    // Fog sends everything to the CPU, for one reason.
    scene.environment.fog = Some(trino_scene::FogDef {
        color: [0, 0, 0],
        near: 5.0,
        far: 30.0,
    });
    let fog = estimate(&scene, &lib, &meshes, Target::N64);
    assert_eq!(fog.rsp_draws, 0);
    assert_eq!(fog.off_rsp, vec![(trino_core::RspReject::Fog, 3)]);
    assert!(fog.n64_cpu_us() > b.n64_cpu_us());
    // The PC has no RSP: its GPU T&L takes what `pc_gpu_check` accepts
    // (fog included) and the translucent rock stays CPU work.
    let pc = estimate(&scene, &lib, &meshes, Target::Pc);
    assert_eq!((pc.rsp_draws, pc.pc_gpu_draws, pc.cpu_draws), (0, 2, 1));
    assert_eq!(
        pc.off_pc_gpu,
        vec![(trino_core::PcGpuReject::Translucent, 1)]
    );
    assert!(pc.off_rsp.is_empty());
    // The 3DS splits by its own rules (`gpu_check`): fog is the vertex
    // shader's, the flat and the translucent rock stay on the CPU.
    let n3ds = estimate(&scene, &lib, &meshes, Target::N3ds);
    assert_eq!((n3ds.rsp_draws, n3ds.draws), (0, 3));
    assert_eq!(n3ds.gpu_draws + n3ds.cpu_draws, n3ds.draws, "{n3ds:?}");
    assert!(
        n3ds.gpu_draws >= 1,
        "the plain rock takes the GPU: {n3ds:?}"
    );
    let off: u32 = n3ds.off_gpu.iter().map(|(_, n)| n).sum();
    assert_eq!(off, n3ds.cpu_draws, "every CPU draw has its reason");
    assert!(
        n3ds.off_gpu
            .iter()
            .any(|(r, _)| *r == trino_core::render3d::GpuReject::Translucent),
        "{n3ds:?}"
    );
}

#[test]
fn variants_lod_and_impostors_change_each_platforms_frame() {
    let (meshes, lib) = assets();
    let mut scene = Scene::new("lod");
    scene.environment.camera.eye = [0.0, 3.0, -60.0];
    scene.environment.camera.target = [0.0, 1.0, 0.0];
    let mut boar = Entity::new("boar").with_model(ModelComponent {
        lod: vec![trino_scene::LodDef {
            distance: 20.0,
            impostor: Some("impostors/selftest-boar".into()),
            platforms: trino_scene::LodPlatforms {
                n64: Some(trino_scene::LodAt::Never),
                ..Default::default()
            },
            ..Default::default()
        }],
        ..ModelComponent::new("models/c64_boar")
    });
    boar.platforms.n3ds = Some(trino_scene::Variant {
        disabled: true,
        ..Default::default()
    });
    scene.entities.push(boar);
    let pc = estimate(&scene, &lib, &meshes, Target::Pc);
    assert_eq!(
        (pc.draws, pc.billboards, pc.lod),
        (0, 1, [0, 0, 1, 0]),
        "an impostor far away"
    );
    assert_eq!(pc.tris(), 2);
    let n64 = estimate(&scene, &lib, &meshes, Target::N64);
    assert_eq!(
        (n64.draws, n64.billboards),
        (1, 0),
        "no impostor on the N64: the mesh"
    );
    let n3ds = estimate(&scene, &lib, &meshes, Target::N3ds);
    assert_eq!(
        (n3ds.draws, n3ds.billboards),
        (0, 0),
        "not on the 3DS at all"
    );
    // Up close everyone draws the mesh (the 3DS still has none).
    scene.environment.camera.eye = [0.0, 3.0, -6.0];
    let close = estimate(&scene, &lib, &meshes, Target::Pc);
    assert_eq!(
        (close.draws, close.billboards, close.lod),
        (1, 0, [1, 0, 0, 0])
    );
    let _ = Camera3::default();
    let _ = Vec3::ZERO;
}

/// The PC native profile's GPU T&L split (issue #51, Phase 1: decided and
/// reported): the estimate equals what the PC renderer counts drawing the
/// same scene from the same camera, draws and reasons alike, and the CLI
/// prints it.
#[test]
fn pc_budget_reports_the_gpu_split() {
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    let (meshes, lib) = assets();
    for (id, blob) in &meshes {
        renderer.upload_mesh(trino_core::ModelId(*id), blob.clone());
    }
    for file in ["scenes/showcase.scene.ron", "scenes/interior.scene.ron"] {
        let scene = Scene::load(&root().join(file)).unwrap();
        let b = estimate(&scene, &lib, &meshes, Target::Pc);
        let env = &scene.environment;
        let (camera, light) = (env.core_camera(), env.core_light());
        let resolved = resolve_for(&scene, &lib, 0.0, Target::Pc);
        let visible = resolved.visible_cells(&camera, Vec2::new(640.0, 480.0));
        renderer.begin_frame(trino_core::Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&light);
        for item in &resolved.items {
            if !trino_core::vis::in_view(item.cell, visible) {
                continue;
            }
            if let LodPick::Model(id) = item.lod_from(camera.eye) {
                renderer.draw_model_matrix(id, &item.matrix, Material::VertexLit, &item.params)
            }
        }
        renderer.end_frame();
        let tnl = renderer.stats().tnl;
        assert!(b.pc_gpu_draws > 0, "{file}: {b:?}");
        assert_eq!(
            (tnl.pc_gpu_draws, tnl.pc_gpu_tris),
            (b.pc_gpu_draws, b.pc_gpu_tris),
            "{file}"
        );
        let mut off: Vec<(trino_core::PcGpuReject, u32)> = trino_core::PcGpuReject::ALL
            .iter()
            .map(|r| (*r, renderer.pc_gpu_rejects()[r.index()]))
            .filter(|(_, n)| *n > 0)
            .collect();
        off.sort_by(|a, b| b.1.cmp(&a.1).then(a.0.cmp(&b.0)));
        assert_eq!(off, b.off_pc_gpu, "{file}: the same reasons");
        let rest: u32 = b.off_pc_gpu.iter().map(|(_, n)| n).sum();
        assert_eq!(b.pc_gpu_draws + rest, b.draws, "{file}");
        // The split is exclusive, like the consoles': the CPU path draws
        // only what the GPU path rejects.
        assert_eq!(b.cpu_draws, b.draws - b.pc_gpu_draws, "{file}");
        // Each drawn entity has its path; the reasons list their entities.
        assert_eq!(b.paths.len() as u32, b.draws, "{file}");
        let hw = b
            .paths
            .iter()
            .filter(|p| p.path == RenderPath::Hardware)
            .count();
        assert_eq!(hw as u32, b.pc_gpu_draws, "{file}");
        for (r, n) in &b.off_pc_gpu {
            assert_eq!(
                b.off_path(r.label()).len() as u32,
                *n,
                "{file}: {}",
                r.label()
            );
        }
        assert!(
            budget::pc_gpu_line(&b)
                .starts_with(&format!("PC GPU {}/{} draws", b.pc_gpu_draws, b.draws))
        );
    }
    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "scene",
            "budget",
            "scenes/showcase.scene.ron",
            "--platform",
            "pc",
            "--json",
        ])
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .unwrap();
    assert!(out.status.success());
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let pc = &v.as_array().unwrap()[0];
    let scene = Scene::load(&root().join("scenes/showcase.scene.ron")).unwrap();
    let b = estimate(&scene, &lib, &meshes, Target::Pc);
    assert_eq!(pc["platform"], "pc");
    assert_eq!(pc["pc_gpu_draws"], b.pc_gpu_draws);
    assert_eq!(pc["pc_gpu_tris"], b.pc_gpu_tris);
    let reasons = pc["off_pc_gpu"].as_array().unwrap();
    assert_eq!(reasons.len(), b.off_pc_gpu.len());
    assert_eq!(reasons[0]["reason"], b.off_pc_gpu[0].0.label());
    let paths = pc["paths"].as_array().unwrap();
    assert_eq!(paths.len(), b.paths.len());
    let river = paths.iter().find(|p| p["name"] == "river").unwrap();
    assert_eq!(
        (&river["path"], &river["reason"]),
        (&"cpu".into(), &"water surface".into())
    );
    assert_eq!(b.path_of("river"), Some(RenderPath::Cpu("water surface")));
    let ground = paths.iter().find(|p| p["name"] == "ground").unwrap();
    assert_eq!(
        (&ground["path"], &ground["reason"]),
        (&"gpu".into(), &serde_json::Value::Null)
    );
    assert_eq!(RenderPath::Hardware.text(Target::Pc), "GPU");
    assert_eq!(RenderPath::Hardware.text(Target::N64), "RSP");
    assert_eq!(RenderPath::Cpu("fog").text(Target::N64), "CPU: fog");
}

#[test]
fn scene_budget_cli_prints_the_same_numbers() {
    let out = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["scene", "budget", "scenes/showcase.scene.ron", "--json"])
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .unwrap();
    assert!(
        out.status.success(),
        "{}",
        String::from_utf8_lossy(&out.stderr)
    );
    let v: serde_json::Value = serde_json::from_slice(&out.stdout).unwrap();
    let rows = v.as_array().unwrap();
    assert_eq!(rows.len(), 3);
    let (meshes, lib) = assets();
    let scene = Scene::load(&root().join("scenes/showcase.scene.ron")).unwrap();
    let n64 = rows.iter().find(|r| r["platform"] == "n64").unwrap();
    let b = estimate(&scene, &lib, &meshes, Target::N64);
    assert_eq!(n64["draws"], b.draws);
    assert_eq!(n64["rsp_draws"], b.rsp_draws);
    assert_eq!(n64["n64_frame_us"], b.n64_frame_us());
    assert!(!n64["off_rsp"].as_array().unwrap().is_empty());
    let one = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args([
            "scene",
            "budget",
            "scenes/showcase.scene.ron",
            "--platform",
            "3ds",
        ])
        .current_dir(root())
        .output()
        .unwrap();
    let text = String::from_utf8_lossy(&one.stdout);
    assert!(text.contains("3ds") && !text.contains("  n64"), "{text}");
    let bad = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["scene", "budget"])
        .current_dir(root())
        .output()
        .unwrap();
    assert!(!bad.status.success());
}
