//! `cargo xtask model inspect` — the art acceptance gate — through the
//! real binary: glTF and fake-object masters bake like the engine bakes
//! them, and the report (text and `--json`) carries the numbers an artist
//! or agent gates on: triangles, joints, clips, bounds and the N64 cost.

use std::path::PathBuf;
use std::process::{Command, Output};

use serde_json::Value;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn inspect(args: &[&str]) -> Output {
    Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["model", "inspect"])
        .args(args)
        .current_dir(root())
        .env_remove("TRINO_PROJECT")
        .output()
        .expect("spawn xtask")
}

fn json(args: &[&str]) -> (bool, Value) {
    let mut all = args.to_vec();
    all.push("--json");
    let o = inspect(&all);
    let v: Value = serde_json::from_slice(&o.stdout)
        .unwrap_or_else(|e| panic!("{e}: {}", String::from_utf8_lossy(&o.stdout)));
    (o.status.success(), v)
}

fn clip<'a>(v: &'a Value, name: &str) -> &'a Value {
    v["clips"]
        .as_array()
        .unwrap()
        .iter()
        .find(|c| c["name"] == name)
        .unwrap_or_else(|| panic!("no clip `{name}` in {v}"))
}

/// The engine's own bake of the same file, for cross-checking the report.
fn baked(path: &str, opts: &trino_asset_pipeline::ModelOptions) -> Vec<u8> {
    trino_asset_pipeline::bake_model(&root().join(path), opts).unwrap()
}

#[test]
fn static_glb_reports_its_mesh_and_n64_cost() {
    // A prop standing on y = 0 (the centered cube would warn about it).
    let file = "assets/shared/models/castle64/boar.glb";
    let (ok, v) = json(&[file]);
    assert!(ok, "{v}");
    assert_eq!(v["ok"], true);
    let blob = baked(file, &Default::default());
    let mesh = trino_core::Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(v["bytes"], blob.len());
    assert_eq!(v["vertices"], mesh.vertex_count);
    assert_eq!(v["triangles"], mesh.index_count / 3);
    assert_eq!(v["joints"], 0);
    assert_eq!(v["clips"].as_array().unwrap().len(), 0);
    let expect_us = trino_core::perf::N64_COST
        .mesh_draw_us(mesh.vertex_count as u32, (mesh.index_count / 3) as u32);
    assert_eq!(v["n64_draw_us"], expect_us);
    let pct = v["n64_frame_percent"].as_f64().unwrap();
    let want = expect_us as f64 / trino_core::perf::FRAME_30FPS_US as f64 * 100.0;
    assert!((pct - want).abs() < 0.06, "{pct} vs {want}");
    let size: Vec<f64> = v["size"]
        .as_array()
        .unwrap()
        .iter()
        .map(|x| x.as_f64().unwrap())
        .collect();
    let (_, radius) = mesh.bounds().unwrap();
    assert!((v["radius"].as_f64().unwrap() - radius as f64).abs() < 1e-4);
    assert!(size.iter().all(|&s| s > 0.0), "{size:?}");
    assert!(
        radius as f64 >= size.iter().cloned().fold(0.0, f64::max) / 2.0 - 1e-4,
        "the bounding sphere holds the box"
    );
    assert_eq!(v["warnings"].as_array().unwrap().len(), 0);
    assert_eq!(v["errors"].as_array().unwrap().len(), 0);

    let o = inspect(&[file]);
    assert!(o.status.success());
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(
        t.contains(&format!(
            "{} triangles, {} vertices",
            mesh.index_count / 3,
            mesh.vertex_count
        )),
        "{t}"
    );
    assert!(t.contains("N64 cost estimate"), "{t}");
    assert!(t.contains("OK — bakes for PC, N64 and 3DS"), "{t}");
    assert!(
        !t.contains("skeleton"),
        "a static mesh has no skeleton line"
    );
}

#[test]
fn animated_glb_reports_its_skeleton_and_clips() {
    let file = "assets/shared/models/showcase/robot.glb";
    let (ok, v) = json(&[file]);
    assert!(ok, "{v}");
    let joints = v["joints"].as_u64().unwrap();
    assert!(joints >= 2, "{v}");
    assert_eq!(v["blended_vertices"], 0, "rigid by default");
    let names: Vec<&str> = v["clips"]
        .as_array()
        .unwrap()
        .iter()
        .map(|c| c["name"].as_str().unwrap())
        .collect();
    let mut want = trino_asset_pipeline::clip_names(&root().join(file));
    want.sort();
    let mut got: Vec<String> = names.iter().map(|s| s.to_string()).collect();
    got.sort();
    assert_eq!(got, want, "every clip is named");
    // Default 15 fps sampling: frames = seconds * fps + 1.
    for c in v["clips"].as_array().unwrap() {
        let (s, f) = (
            c["seconds"].as_f64().unwrap(),
            c["frames"].as_u64().unwrap(),
        );
        assert_eq!(f, (s * 15.0).round() as u64 + 1, "{c}");
        assert_eq!(c["loop"], true, "clips loop by default");
    }

    // --fps resamples; --once makes a clip hold its last frame.
    let first = names[0];
    let (_, v30) = json(&[file, "--fps", "30", "--once", first]);
    let c15 = clip(&v, first);
    let c30 = clip(&v30, first);
    assert_eq!(c30["seconds"], c15["seconds"]);
    assert_eq!(
        c30["frames"].as_u64().unwrap(),
        (c30["seconds"].as_f64().unwrap() * 30.0).round() as u64 + 1
    );
    assert_eq!(c30["loop"], false, "--once {first}");
    let other = names.iter().find(|n| **n != first).unwrap();
    assert_eq!(clip(&v30, other)["loop"], true);

    let o = inspect(&[file, "--once", first]);
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(
        t.contains(&format!("rigid skeleton: {joints} joint(s)")),
        "{t}"
    );
    assert!(t.contains(&format!("clip `{first}`:")), "{t}");
    assert!(t.contains("plays once") && t.contains("loops"), "{t}");
}

#[test]
fn smooth_flag_keeps_two_joint_weights() {
    let file = "assets/shared/models/proxies/selftest-strip.gltf";
    let (_, rigid) = json(&[file]);
    let (ok, smooth) = json(&[file, "--smooth"]);
    assert!(ok, "{smooth}");
    assert_eq!(rigid["blended_vertices"], 0);
    let blended = smooth["blended_vertices"].as_u64().unwrap();
    assert!(blended >= 1, "{smooth}");
    assert!(
        smooth["bytes"].as_u64().unwrap() > rigid["bytes"].as_u64().unwrap(),
        "SKW2 adds a section"
    );
    // A skeleton with no clips is flagged, not rejected.
    assert!(
        smooth["warnings"]
            .as_array()
            .unwrap()
            .iter()
            .any(|w| w.as_str().unwrap().contains("skeleton without clips")),
        "{smooth}"
    );
    let o = inspect(&[file, "--smooth"]);
    let t = String::from_utf8_lossy(&o.stdout);
    assert!(
        t.contains(&format!(
            "smooth skeleton: {} joint(s), {blended} vertices blend two joints",
            smooth["joints"]
        )),
        "{t}"
    );
}

#[test]
fn fake_objects_bake_and_heavy_or_centered_ones_warn() {
    let (ok, v) = json(&["assets/shared/models/proxies/crate.prim.ron"]);
    assert!(ok, "{v}");
    assert!(v["triangles"].as_u64().unwrap() >= 12, "{v}");

    // A dense sphere around its center: over a quarter of the N64 frame,
    // and its origin is not at its base.
    let tmp = tempfile::tempdir().unwrap();
    let ball = tmp.path().join("ball.prim.ron");
    std::fs::write(
        &ball,
        "(shape: Sphere(radius: 1.0), segments: 48, pivot: Center)",
    )
    .unwrap();
    let (ok, v) = json(&[ball.to_str().unwrap()]);
    assert!(ok, "warnings do not fail the gate: {v}");
    let warnings: Vec<&str> = v["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|w| w.as_str().unwrap())
        .collect();
    assert!(
        warnings.iter().any(|w| w.contains("of a 30 fps frame")),
        "{warnings:?}"
    );
    assert!(
        warnings
            .iter()
            .any(|w| w.contains("origin is not at the base")),
        "{warnings:?}"
    );
    let min_y = v["min"][1].as_f64().unwrap();
    assert!((min_y + 1.0).abs() < 0.01, "{min_y}");
    let o = inspect(&[ball.to_str().unwrap()]);
    assert!(String::from_utf8_lossy(&o.stdout).contains("  warning: "));

    // A relative path resolves against the working directory.
    let rel = Command::new(env!("CARGO_BIN_EXE_xtask"))
        .args(["model", "inspect", "ball.prim.ron", "--json"])
        .current_dir(tmp.path())
        .env_remove("TRINO_PROJECT")
        .output()
        .unwrap();
    assert!(
        rel.status.success(),
        "{}",
        String::from_utf8_lossy(&rel.stderr)
    );
}

#[test]
fn bad_invocations_and_files_fail() {
    let usage = inspect(&[]);
    assert!(!usage.status.success());
    assert!(String::from_utf8_lossy(&usage.stderr).contains("cargo xtask model inspect <file"));

    let cube = "assets/shared/models/cube.glb";
    for (args, msg) in [
        (&[cube, "--wat"][..], "unknown flag `--wat`"),
        (&[cube, "--fps"], "--fps needs a number"),
        (&[cube, "--fps", "fast"], "--fps needs a number"),
    ] {
        let o = inspect(args);
        assert!(!o.status.success(), "{args:?}");
        assert!(
            String::from_utf8_lossy(&o.stderr).contains(msg),
            "{args:?}: {}",
            String::from_utf8_lossy(&o.stderr)
        );
    }

    let (ok, v) = json(&["/no/such/model.glb"]);
    assert!(!ok);
    assert_eq!(v["ok"], false);
    assert!(v["error"].as_str().unwrap().contains("model.glb"), "{v}");
    let o = inspect(&["/no/such/model.glb"]);
    assert!(!o.status.success());
    assert!(String::from_utf8_lossy(&o.stderr).starts_with("error: "));

    // Not a model at all.
    let tmp = tempfile::tempdir().unwrap();
    let junk = tmp.path().join("junk.glb");
    std::fs::write(&junk, b"not a gltf").unwrap();
    let (ok, v) = json(&[junk.to_str().unwrap()]);
    assert!(!ok && v["ok"] == false, "{v}");
}

#[test]
fn color_slots_are_counted_per_slot() {
    let file = "assets/shared/models/proxies/selftest-slots.gltf";
    let (ok, v) = json(&[file]);
    assert!(ok, "{v}");
    // Three quads in slots 0, 1 and 4.
    assert_eq!(
        v["slot_vertices"],
        serde_json::json!([4, 4, 0, 0, 4]),
        "{v}"
    );
    let t = String::from_utf8_lossy(&inspect(&[file]).stdout).to_string();
    assert!(
        t.contains("color slots: 4 vertices keep their color; slots 1-4 have 4 / 0 / 0 / 4"),
        "{t}"
    );
    // No `_SLOT`: no slot report.
    let (_, cube) = json(&["assets/shared/models/cube.glb"]);
    assert_eq!(cube["slot_vertices"], serde_json::Value::Null);
}

/// `--anim`: every clip's measurements, in text and JSON — the showcase
/// robot's walk skates and dips under the floor, and the report says so.
#[test]
fn anim_flag_measures_every_clip() {
    let (ok, v) = json(&["assets/shared/models/showcase/robot.glb", "--anim"]);
    assert!(ok, "warnings don't fail the gate: {v}");
    let anim = v["anim"].as_array().expect("anim measurements");
    let walk = anim.iter().find(|c| c["name"] == "walk").unwrap();
    assert!(walk["foot_slide"].as_f64().unwrap() > 0.05, "{walk}");
    assert!(walk["lowest"].as_f64().unwrap() < -0.02, "{walk}");
    assert_eq!(walk["feet"].as_array().unwrap().len(), 2);
    let idle = anim.iter().find(|c| c["name"] == "idle").unwrap();
    assert_eq!(idle["warnings"].as_array().unwrap().len(), 0, "{idle}");
    let warnings = v["warnings"].to_string();
    assert!(
        warnings.contains("clip `walk`: a planted foot slides"),
        "{warnings}"
    );
    // Without the flag nothing is measured.
    let (_, plain) = json(&["assets/shared/models/showcase/robot.glb"]);
    assert!(plain["anim"].is_null());
    let o = inspect(&["assets/shared/models/showcase/robot.glb", "--anim"]);
    let text = String::from_utf8_lossy(&o.stdout);
    assert!(text.contains("clip `walk`: stride"), "{text}");
}

/// `model pose` writes a static model of a clip's pose into the project
/// and declares it; bad invocations say why.
#[test]
fn model_pose_exports_a_static_model() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path();
    std::fs::create_dir_all(dir.join("assets/shared/models/showcase")).unwrap();
    std::fs::copy(
        root().join("assets/shared/models/showcase/robot.glb"),
        dir.join("assets/shared/models/showcase/robot.glb"),
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/manifest.toml"),
        "version = 1\n\n[models.robot]\nfile = \"models/showcase/robot.glb\"\n",
    )
    .unwrap();
    std::fs::write(
        dir.join("trino.toml"),
        "[project]\nname = \"r\"\ntitle = \"R\"\n\n[n64]\npackage = \"r\"\n\n[3ds]\npackage = \"r\"\nbin = \"r\"\n",
    )
    .unwrap();
    let p = dir.to_str().unwrap();
    let run = |args: &[&str]| {
        Command::new(env!("CARGO_BIN_EXE_xtask"))
            .args(args)
            .current_dir(root())
            .env_remove("TRINO_PROJECT")
            .output()
            .unwrap()
    };
    let o = run(&[
        "model",
        "pose",
        "models/robot",
        "--clip",
        "wave",
        "--time",
        "0.6",
        "--name",
        "robot-wave",
        "--project",
        p,
    ]);
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    let out = String::from_utf8_lossy(&o.stdout);
    assert!(
        out.contains("wrote assets/shared/models/poses/robot-wave.gltf"),
        "{out}"
    );
    assert!(out.contains("declared [models.robot-wave]"), "{out}");
    let file = dir.join("assets/shared/models/poses/robot-wave.gltf");
    let (ok, v) = json(&[file.to_str().unwrap()]);
    assert!(ok);
    assert_eq!(v["joints"], 0, "static: no skeleton");
    let robot = json(&["assets/shared/models/showcase/robot.glb"]).1;
    assert_eq!(v["triangles"], robot["triangles"]);
    for (args, needle) in [
        (
            &["model", "pose", "models/robot", "--project", p][..],
            "needs --name",
        ),
        (
            &[
                "model",
                "pose",
                "models/robot",
                "--clip",
                "fly",
                "--name",
                "x",
                "--project",
                p,
            ],
            "clip `fly`",
        ),
        (
            &[
                "model",
                "pose",
                "models/robot",
                "--time",
                "soon",
                "--name",
                "x",
                "--project",
                p,
            ],
            "--time",
        ),
        (
            &[
                "model",
                "pose",
                "models/ghost",
                "--name",
                "x",
                "--project",
                p,
            ],
            "not in the manifest",
        ),
        (&["model"], "cargo xtask model pose"),
    ] {
        let o = run(args);
        assert!(!o.status.success(), "{args:?}");
        let err = String::from_utf8_lossy(&o.stderr);
        assert!(err.contains(needle), "{args:?}: {err}");
    }
}
