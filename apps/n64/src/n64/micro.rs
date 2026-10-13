//! Micro perf scenes (ungated): the T&L's **fixed costs** on the console
//! clock, which the per-model cost-model fit cannot separate (ADR 0006 §3).
//! Each scene is a synthetic draw set built in code, measured like the
//! castle64 scenes (`TRINO_PERF_SCENE:micro-*`); `cargo xtask test n64`
//! keeps them out of the gates, checks their work counters (so a number
//! never comes from the wrong T&L path) and prints the derived per-draw,
//! per-vertex and per-triangle costs (`xtask/src/perf.rs`, `micro_report`).
//!
//! - `micro-draws`: 64 draws of a 1-triangle mesh on screen
//!   (`draw_model_matrix`, a precomputed matrix);
//! - `micro-culled`: the same 64 draws behind the camera (bounding-sphere
//!   cull — the floor of a draw);
//! - `micro-trs` / `micro-yaw`: the 64 draws through `draw_model` with a
//!   zero rotation / a yaw (`Transform3::matrix`'s cost);
//! - `micro-gouraud` / `micro-unlit` / `micro-flat`: one draw of a 16×16
//!   vertex grid (256 vertices, 450 triangles) facing the camera at a
//!   constant depth (no clipping, no splits) — the per-vertex cost by
//!   shading mode;
//! - `micro-tris`: the same grid with every triangle listed twice (same
//!   vertices, 900 triangles): the per-triangle cost, and a 900-triangle
//!   batch for sort + submit.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, TMDL_MAGIC};
use trino_core::{
    Color, Mat34, Material, ModelId, ModelParams, Renderer, Shading, Transform3, Vec3,
};
use trino_platform_n64::{N64Renderer, runtime};

const TRI: ModelId = ModelId::from_path("micro/tri");
const GRID: ModelId = ModelId::from_path("micro/grid");
const GRID2: ModelId = ModelId::from_path("micro/grid2");

/// Grid side in vertices. At most 16: two rows (32 vertices) fit the
/// T&L's 32-slot direct-mapped vertex cache, so every vertex is
/// transformed exactly once.
const GRID_N: usize = 16;
/// 64 small draws in an 8×8 block.
const DRAWS: usize = 64;

/// TMDL blob (see `trino_core::render3d`): positions, normals (all facing
/// the camera, -Z), colors, indices, plus a `BNDS` bounding sphere so the
/// whole-mesh cull applies.
fn tmdl(positions: &[Vec3], indices: &[u16], center: Vec3, radius: f32) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(TMDL_MAGIC);
    b.extend_from_slice(&(positions.len() as u32).to_le_bytes());
    b.extend_from_slice(&(indices.len() as u32).to_le_bytes());
    for p in positions {
        for c in [p.x, p.y, p.z] {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for _ in positions {
        for c in [0.0f32, 0.0, -1.0] {
            b.extend_from_slice(&c.to_le_bytes());
        }
    }
    for i in 0..positions.len() {
        b.extend_from_slice(&[(i * 37) as u8, (i * 91) as u8, 200, 255]);
    }
    for i in indices {
        b.extend_from_slice(&i.to_le_bytes());
    }
    b.extend_from_slice(b"BNDS");
    b.extend_from_slice(&16u32.to_le_bytes());
    for c in [center.x, center.y, center.z, radius] {
        b.extend_from_slice(&c.to_le_bytes());
    }
    b
}

/// The camera looks down +Z from the origin, so view space = world space
/// and a front face (CCW on screen) goes "up, then right" from its first
/// vertex.
fn register_meshes(r: &mut N64Renderer) -> bool {
    let tri = tmdl(
        &[
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(0.0, 0.1, 0.0),
            Vec3::new(0.1, 0.0, 0.0),
        ],
        &[0, 1, 2],
        Vec3::new(0.05, 0.05, 0.0),
        0.08,
    );
    // ~100 px wide at z = 4 (focal ≈ 208 px): small enough that the RDP
    // never back-pressures the submit loop.
    let step = 0.128f32;
    let half = step * (GRID_N - 1) as f32 * 0.5;
    let mut pos = Vec::new();
    for y in 0..GRID_N {
        for x in 0..GRID_N {
            pos.push(Vec3::new(
                x as f32 * step - half,
                y as f32 * step - half,
                0.0,
            ));
        }
    }
    let mut idx = Vec::new();
    let mut idx2 = Vec::new();
    for y in 0..GRID_N - 1 {
        for x in 0..GRID_N - 1 {
            let i = (y * GRID_N + x) as u16;
            let n = GRID_N as u16;
            for t in [[i, i + n, i + 1], [i + 1, i + n, i + n + 1]] {
                idx.extend_from_slice(&t);
                idx2.extend_from_slice(&t);
                idx2.extend_from_slice(&t);
            }
        }
    }
    let r_grid = half * 1.5;
    r.register_mesh(TRI.0, tri)
        && r.register_mesh(GRID.0, tmdl(&pos, &idx, Vec3::ZERO, r_grid))
        && r.register_mesh(GRID2.0, tmdl(&pos, &idx2, Vec3::ZERO, r_grid))
}

#[derive(Clone, Copy)]
enum Set {
    Draws,
    Culled,
    Trs,
    Yaw,
    Grid(ModelId, Shading),
}

const SCENES: &[(&str, Set)] = &[
    ("micro-draws", Set::Draws),
    ("micro-culled", Set::Culled),
    ("micro-trs", Set::Trs),
    ("micro-yaw", Set::Yaw),
    ("micro-gouraud", Set::Grid(GRID, Shading::Gouraud)),
    ("micro-unlit", Set::Grid(GRID, Shading::Unlit)),
    ("micro-flat", Set::Grid(GRID, Shading::Flat)),
    ("micro-tris", Set::Grid(GRID2, Shading::Gouraud)),
];

/// Position of small draw `k` (8×8 block at z = 6, or behind the camera).
fn slot(k: usize, z: f32) -> Vec3 {
    Vec3::new((k % 8) as f32 * 0.35 - 1.3, (k / 8) as f32 * 0.35 - 1.3, z)
}

fn translation(p: Vec3) -> Mat34 {
    let mut m = Mat34::IDENTITY;
    m.m[0][3] = p.x;
    m.m[1][3] = p.y;
    m.m[2][3] = p.z;
    m
}

fn render(r: &mut N64Renderer, set: Set) {
    r.begin_frame(Color::BLACK);
    r.set_camera(&Camera3 {
        eye: Vec3::ZERO,
        target: Vec3::new(0.0, 0.0, 1.0),
        ..Camera3::default()
    });
    r.set_light(&DEFAULT_LIGHT);
    let params = ModelParams::DEFAULT;
    match set {
        Set::Draws | Set::Culled => {
            let z = if matches!(set, Set::Draws) { 6.0 } else { -6.0 };
            for k in 0..DRAWS {
                let m = translation(slot(k, z));
                r.draw_model_matrix(TRI, &m, Material::VertexLit, &params);
            }
        }
        Set::Trs | Set::Yaw => {
            let yaw = if matches!(set, Set::Yaw) { 0.5 } else { 0.0 };
            for k in 0..DRAWS {
                let t = Transform3 {
                    position: slot(k, 6.0),
                    rotation: Vec3::new(0.0, yaw, 0.0),
                    ..Transform3::default()
                };
                r.draw_model(TRI, &t, Material::VertexLit, &params);
            }
        }
        Set::Grid(model, shading) => {
            let m = translation(Vec3::new(0.0, 0.0, 4.0));
            let p = ModelParams {
                shading,
                ..ModelParams::DEFAULT
            };
            r.draw_model_matrix(model, &m, Material::VertexLit, &p);
        }
    }
    r.end_frame();
}

/// Run every micro scene and print its report. Out of line: its frame is
/// gone before the caller's next step (64 KB stack).
#[inline(never)]
pub fn run(r: &mut N64Renderer, frames: u32) {
    if !register_meshes(r) {
        runtime::log("TRINO_PERF_MICRO: mesh blobs rejected\n");
        return;
    }
    for &(name, set) in SCENES {
        let mut times = trino_core::perf::FrameTimes::default();
        let v0 = runtime::vblanks();
        r.start_profile();
        for _ in 0..frames {
            let t0 = runtime::now_us();
            render(r, set);
            times.render += runtime::now_us().wrapping_sub(t0);
        }
        times.vblanks = runtime::vblanks().wrapping_sub(v0);
        if let Some(p) = r.take_profile() {
            let mut report = String::new();
            let _ = p.write_report(name, &times, &mut report);
            runtime::log(&report);
        }
    }
}
