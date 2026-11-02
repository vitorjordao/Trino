//! Evidence for issue #138 (graph tables and crowd instances), for review.
//! Not a gate (the gates are `graph_crowd`, `anim_tables` and
//! `every_graph_plays_the_same_from_its_tables`):
//!
//! ```sh
//! cargo test -p xtask --test graph_crowd_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/graph138/`:
//! - `crowd.png`: 16 striders, each with its own instance of
//!   `assets/anim/strider_layers.anim.ron` (its own speed, aim, shots and
//!   hops), at four moments (columns). Rows: an `AnimGraphInstance` over
//!   the blob (every graph before #138), the same over the decoded tables,
//!   a crowd instance (`GraphInstance<3, 3, 0>`: no event buffer) over the
//!   tables, and the per-pixel difference of the first and the third (black:
//!   identical);
//! - `no_inertia.png`: the same crowd as `GraphInstance<3, 0, 0>` (no
//!   inertia slots: an inertialized hop crossfades instead) beside the full
//!   instance at the moment hops run, with their difference.

use std::path::{Path, PathBuf};

use trino_core::anim::{AnimGraph, AnimGraphInstance, GraphInstance, GraphRef, GraphTables};
use trino_core::mem::Arena;
use trino_core::{Camera3, Color, Material, ModelId, ModelParams, Renderer, Transform3, Vec3};
use trino_platform_pc::{PcRenderer, SimProfile};

const STRIDER: ModelId = ModelId::from_path("models/strider");
const FLOOR: ModelId = ModelId::from_path("models/wall");
const CROWD: usize = 16;
/// Updates (60 Hz) at which each column is taken.
const MOMENTS: [usize; 4] = [30, 75, 120, 165];
const SIZE: (u32, u32) = (480, 300);

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

/// Rows of frames of one size, as one sheet.
fn sheet(rows: &[Vec<Vec<u8>>], w: u32, h: u32) -> (u32, u32, Vec<u8>) {
    let cols = rows[0].len() as u32;
    let (sw, sh) = (w * cols, h * rows.len() as u32);
    let mut out = vec![0u8; (sw * sh * 4) as usize];
    for (r, row) in rows.iter().enumerate() {
        for (c, f) in row.iter().enumerate() {
            for y in 0..h {
                let src = (y * w * 4) as usize;
                let dst = (((r as u32 * h + y) * sw + c as u32 * w) * 4) as usize;
                out[dst..dst + (w * 4) as usize].copy_from_slice(&f[src..src + (w * 4) as usize]);
            }
        }
    }
    (sw, sh, out)
}

/// |a − b| per channel, ×8 so a 1/255 difference shows; opaque.
fn diff(a: &[u8], b: &[u8]) -> (Vec<u8>, usize) {
    let mut n = 0;
    let out = a
        .chunks(4)
        .zip(b.chunks(4))
        .flat_map(|(p, q)| {
            let d = [0, 1, 2].map(|k| (p[k].abs_diff(q[k]) as u32 * 8).min(255) as u8);
            if d != [0; 3] {
                n += 1;
            }
            [d[0], d[1], d[2], 255]
        })
        .collect();
    (out, n)
}

/// Player `k`'s parameters at update `f` (the same for every variant).
fn drive(k: usize, f: usize) -> [(usize, f32); 4] {
    let speed = ((k * 7 + f / 30) % 11) as f32 * 0.2;
    let aim = ((k * 5 + f / 20) % 9) as f32 * 0.25 - 1.0;
    [
        (0, speed),
        (1, aim),
        (2, ((k * 3 + f / 25) % 5) as f32 * 0.4 - 0.8),
        (3, if k.is_multiple_of(3) { 0.0 } else { 1.0 }),
    ]
}

/// A crowd of `GraphInstance<L, I, E>` over `g`: its frames at each moment.
fn crowd<const L: usize, const I: usize, const E: usize>(
    r: &mut PcRenderer,
    g: GraphRef,
    fire: usize,
    hop: usize,
) -> Vec<Vec<u8>> {
    let mut crowd = [GraphInstance::<L, I, E>::new(g); CROWD];
    let mut frames = Vec::new();
    for f in 0..=*MOMENTS.last().unwrap() {
        for (k, i) in crowd.iter_mut().enumerate() {
            for (p, v) in drive(k, f) {
                i.set_float(p, v);
            }
            if (f + k * 13) % 50 == 0 {
                i.set_trigger(fire);
            }
            if (f + k * 17) % 70 == 0 {
                i.set_trigger(hop);
            }
            i.update(g, 1.0 / 60.0);
        }
        if MOMENTS.contains(&f) {
            r.begin_frame(Color::rgb(120, 160, 210));
            r.set_camera(&Camera3 {
                eye: Vec3::new(0.0, 4.4, 7.6),
                target: Vec3::new(0.0, 0.4, -0.3),
                fov_y: 0.8,
            });
            r.set_light(&trino_core::DEFAULT_LIGHT);
            r.draw_model(
                FLOOR,
                &Transform3 {
                    position: Vec3::new(0.0, -0.05, 0.0),
                    scale: Vec3::new(9.0, 0.1, 6.0),
                    ..Transform3::default()
                },
                Material::VertexLit,
                &ModelParams::tinted(Color::rgb(110, 150, 100)),
            );
            for (k, i) in crowd.iter().enumerate() {
                let pose = i.pose(g);
                let anim = r.push_pose(&pose);
                let (x, z) = ((k % 4) as f32 * 1.9 - 2.85, (k / 4) as f32 * 1.4 - 2.1);
                r.draw_model(
                    STRIDER,
                    &Transform3 {
                        position: Vec3::new(x, 0.0, z),
                        rotation: Vec3::new(0.0, 0.4, 0.0),
                        ..Transform3::default()
                    },
                    Material::VertexLit,
                    &ModelParams {
                        anim: Some(anim),
                        ..ModelParams::DEFAULT
                    },
                );
            }
            r.end_frame();
            frames.push(r.read_offscreen());
        }
    }
    frames
}

#[test]
#[ignore = "evidence: writes target/evidence/graph138"]
fn graph_crowd_evidence() {
    let root = root();
    let out = root.join("target/evidence/graph138");
    let baked = root.join("target/assets/pc-graph138-shots");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(r) => r,
        Err(e) => {
            eprintln!("skipping: {e}");
            return;
        }
    };
    for m in assets.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    let tang = assets
        .blobs
        .iter()
        .find(|b| b.id == trino_core::asset_id("anim/strider_layers"))
        .expect("anim/strider_layers baked")
        .bytes
        .clone();
    r.set_internal_size(SIZE.0, SIZE.1);
    let g = AnimGraph::parse(&tang).unwrap();
    for (i, n) in ["speed", "aim_x", "aim_y", "aiming"].iter().enumerate() {
        assert_eq!(g.param_named(n), Some(i), "{n}");
    }
    let (fire, hop) = (
        g.param_named("fire").unwrap(),
        g.param_named("hop").unwrap(),
    );
    let mut memory = vec![std::mem::MaybeUninit::uninit(); GraphTables::bytes_for(&g)];
    let arena = Arena::new(&mut memory);
    let t = GraphTables::decode_in(&g, &arena).unwrap();

    let blob = crowd::<4, 4, 1>(&mut r, (&g).into(), fire, hop);
    let tables = crowd::<4, 4, 1>(&mut r, (&t).into(), fire, hop);
    let slim = crowd::<3, 3, 0>(&mut r, (&t).into(), fire, hop);
    let bare = crowd::<3, 0, 0>(&mut r, (&t).into(), fire, hop);
    let (w, h) = SIZE;
    let mut diffs = Vec::new();
    for k in 0..MOMENTS.len() {
        assert!(
            blob[k] == tables[k],
            "moment {k}: tables drew another frame"
        );
        let (d, n) = diff(&blob[k], &slim[k]);
        assert_eq!(n, 0, "moment {k}: the crowd instance drew another frame");
        diffs.push(d);
    }
    let (sw, sh, px) = sheet(&[blob.clone(), tables, slim, diffs], w, h);
    write_png(&out.join("crowd.png"), sw, sh, &px);

    // Without inertia slots the hops crossfade: frames apart while a hop
    // runs, the same once it is over.
    let mut moved = 0;
    let mut row = Vec::new();
    for k in 0..MOMENTS.len() {
        let (d, n) = diff(&blob[k], &bare[k]);
        moved += n;
        row.push(d);
    }
    assert!(moved > 0, "the hops never ran");
    let (sw, sh, px) = sheet(&[blob, bare, row], w, h);
    write_png(&out.join("no_inertia.png"), sw, sh, &px);
    println!(
        "wrote {} (instances: AnimGraphInstance {} B, GraphInstance<3,3,0> {} B, <3,0,0> {} B; tables {} B)",
        out.display(),
        std::mem::size_of::<AnimGraphInstance>(),
        std::mem::size_of::<GraphInstance<3, 3, 0>>(),
        std::mem::size_of::<GraphInstance<3, 0, 0>>(),
        arena.used()
    );
}
