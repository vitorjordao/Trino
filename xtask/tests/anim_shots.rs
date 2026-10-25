//! Evidence for issue #65, for side-by-side review. Not a gate:
//!
//! ```sh
//! cargo test -p xtask --test anim_shots -- --ignored
//! ```
//!
//! Writes `target/evidence/anim/`:
//! - `ik_step_{off,on}.png`: the strider idling with its left foot over a
//!   20 cm step, without and with foot IK (native profile), and
//!   `ik_step_n64.png` on the N64 profile (its tier: no IK);
//! - `ik_slope_{off,on}.png`: on a 25° sideways slope;
//! - `walk_{constant,root}.png`: eight frames of the walk on one picture,
//!   moved at the clip's mean speed vs carried by its root track (the
//!   planted foot's prints spread vs stack);
//! - `compressed_{raw,anmc}.png`: one pose of the hop from the raw clip and
//!   from its compressed tracks.

use std::path::{Path, PathBuf};

use trino_core::anim::{FootIk, IkPose};
use trino_core::{
    AnimRef, Camera3, ClipId, Color, Mat34, Material, Mesh, ModelId, ModelParams, Renderer,
    Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

const STRIDER: ModelId = ModelId::from_path("models/strider");
const BOX: ModelId = ModelId::from_path("models/wall");
const RAW: ModelId = ModelId(0x5157_0001);

fn renderer(root: &Path, profile: SimProfile) -> Option<(PcRenderer, Vec<u8>)> {
    let baked = root.join("target/assets/pc-anim-shots");
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
    let mut strider = Vec::new();
    for m in assets.models {
        if m.id == STRIDER.0 {
            strider = m.tmdl.clone();
        }
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    r.set_internal_size(480, 360);
    Some((r, strider))
}

/// A slab under each foot: the left one (+X) raised by `step`; with a
/// slope the ground tilts sideways (rising toward +X), one slab.
fn ground(r: &mut PcRenderer, slope_deg: f32, step: f32) {
    let slab = |r: &mut PcRenderer, x: f32, w: f32, h: f32, rot: f32, c: Color| {
        r.draw_model(
            BOX,
            &Transform3 {
                position: Vec3::new(x, h - 0.05, 0.0),
                rotation: Vec3::new(0.0, 0.0, rot),
                scale: Vec3::new(w, 0.1, 1.6),
            },
            Material::VertexLit,
            &ModelParams::tinted(c),
        );
    };
    if slope_deg != 0.0 {
        slab(
            r,
            0.0,
            3.0,
            0.0,
            slope_deg.to_radians(),
            Color::rgb(110, 150, 100),
        );
    } else {
        slab(r, -0.75, 1.5, 0.0, 0.0, Color::rgb(110, 150, 100));
        slab(r, 0.75, 1.5, step, 0.0, Color::rgb(170, 140, 100));
    }
}

/// The strider at the origin, facing +Z; `ik` from the ground under its feet.
fn strider_frame(
    r: &mut PcRenderer,
    mesh: &Mesh,
    anim: AnimRef,
    slope: f32,
    step: f32,
    ik: bool,
    eye: Vec3,
) -> Vec<u8> {
    r.begin_frame(Color::rgb(120, 160, 210));
    r.set_camera(&Camera3 {
        eye,
        target: Vec3::new(0.0, 0.4, 0.0),
        fov_y: 0.9,
    });
    r.set_light(&trino_core::DEFAULT_LIGHT);
    ground(r, slope, step);
    let pose = if ik {
        let chains = mesh.ik_chains().unwrap();
        let mut w = [Mat34::IDENTITY; trino_core::MAX_JOINTS];
        mesh.pose_world(Some(anim), &mut w);
        let foot = |l: usize| {
            let m = w[chains.chain[l][2] as usize];
            Vec3::new(m.m[0][3], m.m[1][3], m.m[2][3])
        };
        let caps = *r.caps();
        FootIk::default().pose(&caps, &Mat34::IDENTITY, [foot(0), foot(1)], |p| {
            Some(p.x * slope.to_radians().tan() + if p.x > 0.0 { step } else { 0.0 })
        })
    } else {
        IkPose::NONE
    };
    r.draw_model(
        STRIDER,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams {
            anim: Some(anim.with_ik(pose)),
            ..ModelParams::DEFAULT
        },
    );
    r.end_frame();
    eprintln!("ik pose {pose:?}");
    r.read_offscreen()
}

#[test]
#[ignore = "evidence: writes target/evidence/anim (needs a GPU)"]
fn anim_evidence_shots() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let out = root.join("target/evidence/anim");
    let Some((mut r, tmdl)) = renderer(&root, SimProfile::Native) else {
        return;
    };
    let (w, h) = r.internal_size();
    let mesh = Mesh::from_tmdl(&tmdl).unwrap();
    let idle = AnimRef::new(ClipId::from_name("idle"), 0.5);
    // The step is under the strider's left (+X); seen from the front.
    let side = Vec3::new(0.5, 0.6, 2.6);
    for (name, ik) in [("ik_step_off", false), ("ik_step_on", true)] {
        let px = strider_frame(&mut r, &mesh, idle, 0.0, 0.2, ik, side);
        write_png(&out.join(format!("{name}.png")), w, h, &px);
    }
    for (name, ik) in [("ik_slope_off", false), ("ik_slope_on", true)] {
        let px = strider_frame(&mut r, &mesh, idle, 25.0, 0.0, ik, side);
        write_png(&out.join(format!("{name}.png")), w, h, &px);
    }
    // The N64 profile: its tier has no IK (FootIk returns NONE).
    if let Some((mut n, _)) = renderer(&root, SimProfile::N64) {
        let (nw, nh) = n.internal_size();
        let px = strider_frame(&mut n, &mesh, idle, 0.0, 0.2, true, side);
        write_png(&out.join("ik_step_n64.png"), nw, nh, &px);
    }

    // The walk: eight frames on one picture, its planted foot's prints.
    let meta = trino_core::anim::ClipMeta::from_tmdl(&tmdl).unwrap();
    let walk = ClipId::from_name("walk");
    let track = meta.roots.root_of(walk).unwrap();
    for (name, carried) in [("walk_constant", false), ("walk_root", true)] {
        r.begin_frame(Color::rgb(120, 160, 210));
        r.set_camera(&Camera3 {
            eye: Vec3::new(3.0, 1.2, 0.6),
            target: Vec3::new(0.0, 0.4, 0.6),
            fov_y: 0.8,
        });
        r.set_light(&trino_core::DEFAULT_LIGHT);
        ground(&mut r, 0.0, 0.0);
        for k in 0..8 {
            let t = k as f32 * 0.5 / 8.0;
            let z = if carried { track.at(t).z } else { 0.8 * t };
            let a = (k as u8) * 20 + 80;
            r.draw_model(
                STRIDER,
                &Transform3 {
                    position: Vec3::new(0.0, 0.0, z),
                    ..Transform3::default()
                },
                Material::VertexLit,
                &ModelParams {
                    anim: Some(AnimRef::new(walk, t)),
                    tint: Color::rgba(255, 255, 255, a),
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.end_frame();
        let px = r.read_offscreen();
        write_png(&out.join(format!("{name}.png")), w, h, &px);
    }

    // Raw vs compressed: the hop mid-air.
    let m = trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let d = &m.models["strider"];
    let src = trino_asset_pipeline::resolve_source(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &d.file,
    )
    .unwrap();
    let mut opts = d.options();
    opts.anim_compression = None;
    let raw = trino_asset_pipeline::bake_model(&src, &opts).unwrap();
    r.upload_mesh(RAW, raw);
    let hop = AnimRef::new(ClipId::from_name("hop"), 0.55);
    for (name, id) in [("compressed_raw", RAW), ("compressed_anmc", STRIDER)] {
        r.begin_frame(Color::rgb(120, 160, 210));
        r.set_camera(&Camera3 {
            eye: Vec3::new(1.6, 1.4, 2.4),
            target: Vec3::new(0.0, 1.0, 0.0),
            fov_y: 0.9,
        });
        r.set_light(&trino_core::DEFAULT_LIGHT);
        r.draw_model(
            id,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                anim: Some(hop),
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        let px = r.read_offscreen();
        write_png(&out.join(format!("{name}.png")), w, h, &px);
    }
    eprintln!("wrote {}", out.display());
}
