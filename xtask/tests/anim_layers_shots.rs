//! Evidence for issue #112, for side-by-side review. Not a gate:
//!
//! ```sh
//! cargo test -p xtask --test anim_layers_shots -- --ignored
//! ```
//!
//! Writes `target/evidence/anim112/`:
//! - `tiers.png`: the strider running while it aims up and to its left
//!   (`assets/anim/strider_layers.anim.ron`), the same pose pushed into each
//!   tier's renderer: everything (the PC, the New 3DS), two layers of six
//!   clips (the Expansion Pak, the 3DS), the base N64's fold (one layer of
//!   three: the arms swing with the run);
//! - `aim_2d.png`: the upper layer's 2D blend space at nine aims (a 3 × 3
//!   grid of yaw × pitch), the legs idle;
//! - `inertial.png`: five frames after the hop cuts in (inertialized),
//!   beside the same frames with a crossfade;
//! - `slope.png`: the idle strider on a 20° slope with foot IK, its feet
//!   flat (height only, #65) and turned onto the ground (`FootSlope`).

use std::path::{Path, PathBuf};

use trino_core::anim::{AnimGraph, AnimGraphInstance, AnimPose, FootIk, FootSlope};
use trino_core::{
    Camera3, Color, HwTier, Mat34, Material, Mesh, ModelId, ModelParams, Renderer, Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const STRIDER: ModelId = ModelId::from_path("models/strider");
const BOX: ModelId = ModelId::from_path("models/wall");

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// Frames of the same size side by side.
fn strip(frames: &[Vec<u8>], w: u32, h: u32) -> Vec<u8> {
    let n = frames.len() as u32;
    let mut out = vec![0u8; (w * n * h * 4) as usize];
    for (k, f) in frames.iter().enumerate() {
        for y in 0..h {
            let src = (y * w * 4) as usize;
            let dst = ((y * w * n + k as u32 * w) * 4) as usize;
            out[dst..dst + (w * 4) as usize].copy_from_slice(&f[src..src + (w * 4) as usize]);
        }
    }
    out
}

struct Shot {
    r: PcRenderer,
    tmdl: Vec<u8>,
    tang: Vec<u8>,
}

fn shot(profile: SimProfile, tier: HwTier) -> Option<Shot> {
    let root = root();
    let baked = root.join("target/assets/pc-anim112-shots");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return None;
        }
    };
    r.set_hw_tier(tier);
    let mut tmdl = Vec::new();
    for m in assets.models {
        if m.id == STRIDER.0 {
            tmdl = m.tmdl.clone();
        }
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    let tang = assets
        .blobs
        .iter()
        .find(|b| b.id == trino_core::asset_id("anim/strider_layers"))
        .expect("anim/strider_layers baked")
        .bytes
        .clone();
    r.set_internal_size(320, 360);
    Some(Shot { r, tmdl, tang })
}

/// The instance after `secs` of `params` (60 Hz).
fn played(g: &AnimGraph, params: &[(&str, f32)], secs: f32) -> AnimGraphInstance {
    let mut i = AnimGraphInstance::new(g);
    for (p, v) in params {
        i.set(g, p, *v);
    }
    for _ in 0..(secs * 60.0) as usize {
        i.update(g, 1.0 / 60.0);
    }
    i
}

fn frame(r: &mut PcRenderer, pose: &AnimPose, eye: Vec3, slope: f32) -> Vec<u8> {
    r.begin_frame(Color::rgb(120, 160, 210));
    r.set_camera(&Camera3 {
        eye,
        target: Vec3::new(0.0, 0.85, 0.0),
        fov_y: 0.9,
    });
    r.set_light(&trino_core::DEFAULT_LIGHT);
    r.draw_model(
        BOX,
        &Transform3 {
            position: Vec3::new(0.0, -0.05, 0.0),
            // Rising toward +X, as `ground(p) = p.x · tan` has it.
            rotation: Vec3::new(0.0, 0.0, -slope.to_radians()),
            scale: Vec3::new(3.0, 0.1, 1.6),
        },
        Material::VertexLit,
        &ModelParams::tinted(Color::rgb(110, 150, 100)),
    );
    let anim = r.push_pose(pose);
    r.draw_model(
        STRIDER,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams {
            anim: Some(anim),
            ..ModelParams::DEFAULT
        },
    );
    r.end_frame();
    r.read_offscreen()
}

const FRONT: Vec3 = Vec3::new(1.2, 1.4, 2.8);

#[test]
#[ignore = "evidence: writes target/evidence/anim112"]
fn layered_pose_evidence() {
    let out = root().join("target/evidence/anim112");
    // The same pose in every tier's renderer.
    let tiers = [
        (SimProfile::Native, HwTier::Base),
        (SimProfile::N3ds, HwTier::Base),
        (SimProfile::N64, HwTier::Expanded),
        (SimProfile::N64, HwTier::Base),
    ];
    let mut frames = Vec::new();
    for (profile, tier) in tiers {
        let Some(mut s) = shot(profile, tier) else {
            return;
        };
        let g = AnimGraph::parse(&s.tang).unwrap();
        let i = played(
            &g,
            &[
                ("speed", 1.6),
                ("aiming", 1.0),
                ("aim_x", -0.8),
                ("aim_y", 0.6),
            ],
            1.3,
        );
        let pose = i.pose(&g);
        let (w, h) = s.r.internal_size();
        let f = frame(&mut s.r, &pose, FRONT, 0.0);
        let caps = s.r.caps().anim_layers;
        eprintln!("{profile:?} {tier:?}: caps {caps:?}");
        assert_eq!(f.len(), (w * h * 4) as usize);
        frames.push(f);
    }
    let (w, h) = (320, 360);
    write_png(&out.join("tiers.png"), w * 4, h, &strip(&frames, w, h));
    // Different tiers draw different poses (or the fold would be silent).
    assert!(frames[0] != frames[3]);

    let Some(mut s) = shot(SimProfile::Native, HwTier::Base) else {
        return;
    };
    let g = AnimGraph::parse(&s.tang).unwrap();
    // The 2D aim space, yaw × pitch.
    let mut grid = Vec::new();
    for pitch in [0.9f32, 0.0, -0.9] {
        let mut row = Vec::new();
        for yaw in [-0.9f32, 0.0, 0.9] {
            let i = played(
                &g,
                &[("aiming", 1.0), ("aim_x", yaw), ("aim_y", pitch)],
                0.4,
            );
            row.push(frame(&mut s.r, &i.pose(&g), Vec3::new(0.0, 1.3, 3.0), 0.0));
        }
        grid.push(strip(&row, w, h));
    }
    write_png(&out.join("aim_2d.png"), w * 3, h * 3, &grid.concat());

    // Inertialized vs crossfaded hop: the graph's own (inertial) and a copy
    // with the transition's mode turned back to a crossfade.
    let def_path = root().join("assets/anim/strider_layers.anim.ron");
    let mut def = trino_scene::anim::AnimGraphDef::load(&def_path).unwrap();
    def.transitions[0].mode = trino_scene::anim::TransitionMode::Crossfade;
    let manifest =
        trino_asset_pipeline::Manifest::load(&root().join("assets/manifest.toml")).unwrap();
    let cross = trino_asset_pipeline::anim_graph::bake(
        &root(),
        &manifest,
        trino_asset_pipeline::Platform::Pc,
        &def,
    )
    .unwrap()
    .unwrap();
    let mut rows = Vec::new();
    for bytes in [&s.tang.clone(), &cross] {
        let g = AnimGraph::parse(bytes).unwrap();
        let mut i = played(&g, &[("speed", 1.6)], 0.5);
        i.set(&g, "hop", 1.0);
        let mut row = Vec::new();
        for _ in 0..5 {
            for _ in 0..3 {
                i.update(&g, 1.0 / 60.0);
            }
            row.push(frame(&mut s.r, &i.pose(&g), Vec3::new(3.0, 1.2, 0.4), 0.0));
        }
        rows.push(strip(&row, w, h));
    }
    write_png(&out.join("inertial.png"), w * 5, h * 2, &rows.concat());

    // Feet on a 20° slope: height only, then turned onto the ground.
    let mesh = Mesh::from_tmdl(&s.tmdl).unwrap();
    let i = played(&g, &[], 0.5);
    let feet = i.feet(&g).unwrap();
    let caps = *s.r.caps();
    let tan = 20f32.to_radians().tan();
    let ik = FootIk::default().pose(&caps, &Mat34::IDENTITY, feet, |p| Some(p.x * tan));
    let n = Vec3::new(-tan, 1.0, 0.0);
    let slope = FootIk::default().slope(&caps, &Mat34::IDENTITY, feet, |_| Some(n));
    assert_ne!(slope, FootSlope::FLAT);
    let _ = mesh;
    let base = i.pose(&g).with_ik(ik);
    let eye = Vec3::new(0.0, 0.9, 3.4);
    let flat = frame(&mut s.r, &base, eye, 20.0);
    let turned = frame(&mut s.r, &base.with_slope(slope), eye, 20.0);
    assert!(flat != turned);
    write_png(
        &out.join("slope.png"),
        w * 2,
        h,
        &strip(&[flat, turned], w, h),
    );
}
