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
//!   batch for sort + submit;
//! - `micro-palette`: a crowd of 16 small slotted figures (6×6 vertices,
//!   50 triangles each), each recolored by its own palette (GitHub #122):
//!   the CPU T&L against the RSP's palette command in the same ROM.

extern crate alloc;

use alloc::string::String;
use alloc::vec::Vec;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Fog, Light, TMDL_MAGIC};
use trino_core::{
    Color, Mat34, Material, ModelId, ModelParams, Renderer, Shading, Transform3, Vec3,
};
use trino_platform_n64::{N64Renderer, runtime};

const TRI: ModelId = ModelId::from_path("micro/tri");
const GRID: ModelId = ModelId::from_path("micro/grid");
const GRID2: ModelId = ModelId::from_path("micro/grid2");
/// An 8×8-vertex grid as wide as [`GRID`] with alternate rows raised: its
/// faces point different ways, for flat shading on the RSP (98 faces fit
/// a flat copy, `RSP_FLAT_SEG_TRIS` · `RSP_MAX_SEGS`).
const BUMPY: ModelId = ModelId::from_path("micro/bumpy");
/// [`GRID`] as a rigid mesh of two joints (left and right halves, the
/// middle column duplicated): the RSP draws it as two parts.
const RIGID: ModelId = ModelId::from_path("micro/rigid");
/// [`GRID`] with color slots (`SLOT`, GitHub #122): slot
/// `(x / 4 + y / 4) % 5`, so every slot 0..=4 shows in bands.
const GRID_SLOTS: ModelId = ModelId::from_path("micro/grid_slots");
/// [`BUMPY`] with slots `(x + y) % 5`: a recolored flat draw (its flat
/// copy carries each face vertex's slot).
const BUMPY_SLOTS: ModelId = ModelId::from_path("micro/bumpy_slots");
/// [`RIGID`] with the same slots: a recolored rigid draw, a part per joint.
const RIGID_SLOTS: ModelId = ModelId::from_path("micro/rigid_slots");
/// castle64's textured self-test card with slots: the textured overlay's
/// palette command.
const CARD_SLOTS: ModelId = ModelId::from_path("micro/card_slots");
/// castle64's self-test card (a 2×2 texel-quadrant texture).
const CARD: ModelId = ModelId::from_path("models/selftest_card");
/// One figure of the `micro-palette` crowd: 6×6 vertices, 50 triangles,
/// slots `(x + y) % 5` (several per face).
const FIGURE: ModelId = ModelId::from_path("micro/figure");
/// Figures in the `micro-palette` crowd (a 4×4 block).
const CROWD: usize = 16;
/// Figure side in vertices.
const FIGURE_N: usize = 6;

/// Grid side in vertices. At most 16: two rows (32 vertices) fit the
/// T&L's 32-slot direct-mapped vertex cache, so every vertex is
/// transformed exactly once.
const GRID_N: usize = 16;
/// 64 small draws in an 8×8 block.
const DRAWS: usize = 64;
/// Triangles of [`GRID2`]: every grid cell's two triangles, twice — the
/// biggest batch of the micro scenes (the caller sizes it first).
pub const BATCH_TRIS: usize = (GRID_N - 1) * (GRID_N - 1) * 4;

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

/// Slot of grid vertex `(x, y)` in [`GRID_SLOTS`] and [`RIGID_SLOTS`].
fn grid_slot(x: usize, y: usize) -> u8 {
    ((x / 4 + y / 4) % 5) as u8
}

/// `blob` with a `SLOT` section (one color slot per vertex) appended.
fn with_slots(mut blob: Vec<u8>, slots: &[u8]) -> Vec<u8> {
    blob.extend_from_slice(b"SLOT");
    blob.extend_from_slice(&(slots.len() as u32).to_le_bytes());
    blob.extend_from_slice(slots);
    blob
}

/// Figure `k`'s palette: four colors of its own (a crowd recolored per
/// character, the accessibility and kit swaps of a real game).
fn crowd_palette(k: usize) -> trino_core::Palette {
    let c = |a: usize, b: usize| (a * 37 + b * 61) as u8;
    [
        Color::rgb(c(k, 1), 255 - c(k, 2), 90),
        Color::rgb(40, c(k, 3), 255 - c(k, 4)),
        Color::rgb(255 - c(k, 5), 200, c(k, 6)),
        Color::rgb(c(k, 7), c(k, 8), c(k, 9)),
    ]
}

/// The palette the RSP checks recolor with: saturated, unlike the grid's
/// own colors, so a missing multiply shows.
const CHECK_PALETTE: trino_core::Palette = [
    Color::rgb(230, 40, 40),
    Color::rgb(40, 200, 60),
    Color::rgb(50, 70, 230),
    Color::rgb(250, 220, 30),
];

/// The camera looks down +Z from the origin, so view space = world space
/// and a front face (CCW on screen) goes "up, then right" from its first
/// vertex.
///
/// With `palette`, the slotted meshes of the palette checks and of
/// `micro-palette` (GitHub #122) instead, castle64's card among them:
/// [`run`] registers those after the batch reserve and forgets them after
/// its scenes ([`forget_palette_meshes`]), so the base N64's heap holds
/// them only while they are used (they took the block the reserve needs
/// when registered at boot).
fn register_meshes(r: &mut N64Renderer, palette: bool) -> bool {
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
    let n = 8usize;
    let step8 = step * (GRID_N - 1) as f32 / (n - 1) as f32;
    let mut bumpy = Vec::new();
    for y in 0..n {
        for x in 0..n {
            let lift = if (x + y) % 2 == 0 { 0.0 } else { -0.12 };
            bumpy.push(Vec3::new(
                x as f32 * step8 - half,
                y as f32 * step8 - half,
                lift,
            ));
        }
    }
    let bumpy_slots: Vec<u8> = (0..n * n).map(|i| ((i % n + i / n) % 5) as u8).collect();
    let mut bidx = Vec::new();
    for y in 0..n - 1 {
        for x in 0..n - 1 {
            let i = (y * n + x) as u16;
            let w = n as u16;
            bidx.extend_from_slice(&[i, i + w, i + 1, i + 1, i + w, i + w + 1]);
        }
    }
    debug_assert_eq!(idx2.len() / 3, BATCH_TRIS);
    let grid_slots: Vec<u8> = (0..GRID_N * GRID_N)
        .map(|i| grid_slot(i % GRID_N, i / GRID_N))
        .collect();
    // The figure: ~0.25 wide at z = 6 (about 9 px), its own grid.
    let fstep = 0.05f32;
    let fhalf = fstep * (FIGURE_N - 1) as f32 * 0.5;
    let mut fpos = Vec::new();
    let mut fslots = Vec::new();
    for y in 0..FIGURE_N {
        for x in 0..FIGURE_N {
            fpos.push(Vec3::new(
                x as f32 * fstep - fhalf,
                y as f32 * fstep - fhalf,
                0.0,
            ));
            fslots.push(((x + y) % 5) as u8);
        }
    }
    let mut fidx = Vec::new();
    for y in 0..FIGURE_N - 1 {
        for x in 0..FIGURE_N - 1 {
            let i = (y * FIGURE_N + x) as u16;
            let w = FIGURE_N as u16;
            fidx.extend_from_slice(&[i, i + w, i + 1, i + 1, i + w, i + w + 1]);
        }
    }
    if palette {
        let figure = with_slots(tmdl(&fpos, &fidx, Vec3::ZERO, fhalf * 1.5), &fslots);
        // castle64's textured card, its vertices in slots 0..=4 in turn.
        let card = r.mesh_blob(CARD).map(|b| {
            let n = trino_core::render3d::Mesh::from_tmdl(&b).map_or(0, |m| m.vertex_count);
            let slots: Vec<u8> = (0..n).map(|i| (i % 5) as u8).collect();
            with_slots(b, &slots)
        });
        return r.register_mesh(
            GRID_SLOTS.0,
            with_slots(tmdl(&pos, &idx, Vec3::ZERO, r_grid), &grid_slots),
        ) && r.register_mesh(
            BUMPY_SLOTS.0,
            with_slots(tmdl(&bumpy, &bidx, Vec3::ZERO, r_grid), &bumpy_slots),
        ) && r.register_mesh(RIGID_SLOTS.0, rigid_grid(&pos, n_grid_half(), r_grid, true))
            && r.register_mesh(FIGURE.0, figure)
            && card.is_some_and(|b| r.register_mesh(CARD_SLOTS.0, b));
    }
    r.register_mesh(TRI.0, tri)
        && r.register_mesh(GRID.0, tmdl(&pos, &idx, Vec3::ZERO, r_grid))
        && r.register_mesh(GRID2.0, tmdl(&pos, &idx2, Vec3::ZERO, r_grid))
        && r.register_mesh(BUMPY.0, tmdl(&bumpy, &bidx, Vec3::ZERO, r_grid))
        && r.register_mesh(RIGID.0, rigid_grid(&pos, n_grid_half(), r_grid, false))
}

/// Give back what `register_meshes(r, true)` took: the meshes and their RSP
/// and flat copies. The renderer frees a copy four frames after its last
/// possible use (its grave): a few empty frames hand the blocks back before
/// the next step allocates (the base N64's water step needs 43 KB).
fn forget_palette_meshes(r: &mut N64Renderer) {
    for m in [GRID_SLOTS, BUMPY_SLOTS, RIGID_SLOTS, FIGURE, CARD_SLOTS] {
        r.unregister_mesh(m.0);
    }
    for _ in 0..6 {
        r.begin_frame(Color::BLACK);
        r.end_frame();
    }
}

#[derive(Clone, Copy)]
enum Set {
    Draws,
    Culled,
    Trs,
    Yaw,
    Grid(ModelId, Shading),
    Crowd,
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
    setup(r);
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
        Set::Crowd => {
            for k in 0..CROWD {
                let at = Vec3::new((k % 4) as f32 * 0.4 - 0.6, (k / 4) as f32 * 0.4 - 0.6, 6.0);
                let p = ModelParams {
                    palette: Some(crowd_palette(k)),
                    ..ModelParams::DEFAULT
                };
                r.draw_model_matrix(FIGURE, &translation(at), Material::VertexLit, &p);
            }
        }
    }
    r.end_frame();
}

/// Run every micro scene and print its report. Out of line: its frame is
/// gone before the caller's next step (64 KB stack).
#[inline(never)]
/// Register the micro meshes and draw the flat grid once on the RSP, at
/// the start of the self-test: its 45 KB flat copy is then cached while
/// the heap is whole, not allocated after the later steps fragmented the
/// base N64's 4 MB (GitHub #110, #66). [`run`] keeps what this made.
#[inline(never)]
pub fn prewarm(r: &mut N64Renderer) -> bool {
    if !register_meshes(r, false) {
        return false;
    }
    let rsp = r.rsp();
    r.set_rsp(true);
    render(r, Set::Grid(GRID, Shading::Flat));
    r.set_rsp(rsp);
    true
}

pub fn run(r: &mut N64Renderer, frames: u32, prewarmed: bool) {
    match r.rsp_selftest() {
        Some(c) if c.r > 100 && c.a > 0 => {
            runtime::log(&alloc::format!("TRINO_RSP:test triangle ok {c:?}\n"))
        }
        c => runtime::log(&alloc::format!(
            "TRINO_TEST_FAIL:rsp: the overlay's test triangle did not render ({c:?})\n"
        )),
    }
    if !prewarmed && !register_meshes(r, false) {
        runtime::log("TRINO_PERF_MICRO: mesh blobs rejected\n");
        return;
    }
    let heap = |when: &str| {
        let (used, total) = runtime::heap();
        runtime::log(&alloc::format!(
            "TRINO_HEAP:{used}/{total} top={} {when} the palette meshes\n",
            runtime::heap_top_free()
        ));
    };
    rsp_checks(r);
    water_checks(r);
    // Same ROM, both paths: the RSP numbers are free of code-layout noise.
    for (suffix, rsp) in [("", false), ("-rsp", true)] {
        r.set_rsp(rsp);
        for &(name, set) in SCENES {
            measure(r, name, suffix, set, frames);
        }
    }
    // Palettes last (GitHub #122): their meshes are registered once the
    // scenes above have grown the batch buffers to their size, and
    // forgotten after, so the base N64's free block is whole again for the
    // water step (registered first, they split it: 81 → 38 KB).
    heap("before");
    match register_meshes(r, true) {
        true => {
            palette_checks(r);
            for (suffix, rsp) in [("", false), ("-rsp", true)] {
                r.set_rsp(rsp);
                measure(r, "micro-palette", suffix, Set::Crowd, frames);
            }
        }
        false => runtime::log("TRINO_TEST_FAIL:rsp palette: the slotted meshes were rejected\n"),
    }
    forget_palette_meshes(r);
    heap("after forgetting");
    r.set_rsp(true);
}

/// Camera + light every micro frame uses.
fn setup(r: &mut N64Renderer) {
    r.set_camera(&Camera3 {
        eye: Vec3::ZERO,
        target: Vec3::new(0.0, 0.0, 1.0),
        ..Camera3::default()
    });
    r.set_light(&DEFAULT_LIGHT);
}

/// A warm key light over a blue ambient: the colored command's case.
const WARM: Light = Light {
    color: Color::rgb(255, 150, 70),
    ambient_color: Color::rgb(90, 110, 200),
    ..DEFAULT_LIGHT
};

/// A glowing tint under [`WARM`].
const GLOW: ModelParams = ModelParams {
    emissive: Color::rgb(40, 0, 90),
    tint: Color::rgb(230, 255, 200),
    ..ModelParams::DEFAULT
};

/// The scene light with fog across the turned grid's depths: the colored
/// overlay's per-vertex fog blend.
const FOGGY: Light = Light {
    fog: Fog {
        color: Color::rgb(132, 178, 226),
        near: 3.6,
        far: 4.4,
    },
    ..DEFAULT_LIGHT
};

/// The bumpy grid turned and squashed (per-axis scale): flat faces through
/// a flat copy baked for its scale ratios.
fn squashed() -> (Mat34, ModelParams) {
    let m = Transform3 {
        position: Vec3::new(0.1, 0.0, 4.0),
        rotation: Vec3::new(0.25, -0.4, 0.15),
        scale: Vec3::new(1.0, 0.7, 0.85),
    }
    .matrix();
    let p = ModelParams {
        shading: Shading::Flat,
        ..ModelParams::DEFAULT
    };
    (m, p)
}

/// The grid turned to show its back (half a turn about Y), drawn
/// double-sided.
fn back_of_grid() -> (Mat34, ModelParams) {
    let m = Transform3 {
        position: Vec3::new(0.0, 0.0, 4.0),
        rotation: Vec3::new(0.0, core::f32::consts::PI, 0.0),
        ..Transform3::default()
    }
    .matrix();
    let p = ModelParams {
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    (m, p)
}

/// The RSP overlay against the CPU path: screen vertices (oracle), then the
/// pixels of the same frame drawn both ways.
fn rsp_checks(r: &mut N64Renderer) {
    setup(r);
    let grid = translation(Vec3::new(0.0, 0.0, 4.0));
    let turned = Transform3 {
        position: Vec3::new(0.2, -0.1, 4.0),
        rotation: Vec3::new(0.3, 0.7, -0.2),
        scale: Vec3::new(0.8, 0.8, 0.8),
    }
    .matrix();
    match r.rsp_oracle(GRID, &turned, &ModelParams::DEFAULT) {
        Ok(m) => runtime::log(&alloc::format!("TRINO_RSP:oracle rotated: {m}\n")),
        Err(m) => runtime::log(&alloc::format!("TRINO_TEST_FAIL:rsp oracle rotated: {m}\n")),
    }
    for (what, params) in [
        ("gouraud", ModelParams::DEFAULT),
        (
            "unlit tinted",
            ModelParams {
                shading: Shading::Unlit,
                tint: Color::rgb(200, 120, 255),
                ..ModelParams::DEFAULT
            },
        ),
    ] {
        match r.rsp_oracle(GRID, &grid, &params) {
            Ok(m) => runtime::log(&alloc::format!("TRINO_RSP:oracle {what}: {m}\n")),
            Err(m) => runtime::log(&alloc::format!("TRINO_TEST_FAIL:rsp oracle {what}: {m}\n")),
        }
    }
    // The colored command (warm light, emissive), and a back face.
    r.set_light(&WARM);
    let colored = r.rsp_oracle(GRID, &grid, &GLOW);
    setup(r);
    let (back, both) = back_of_grid();
    let flipped = r.rsp_oracle(GRID, &back, &both);
    r.set_light(&FOGGY);
    let fogged = r.rsp_oracle(GRID, &turned, &ModelParams::DEFAULT);
    setup(r);
    let (squash, flat) = squashed();
    let faces = r.rsp_oracle(BUMPY, &squash, &flat);
    // Textured: the self-test card (castle64's asset, a 2x2 texel-quadrant
    // texture) facing the camera.
    let card = CARD;
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let textured = r.rsp_oracle(card, &grid, &unlit);
    for (what, res) in [
        ("colored emissive", colored),
        ("double-sided back", flipped),
        ("fog", fogged),
        ("flat squashed", faces),
        ("textured", textured),
    ] {
        match res {
            Ok(m) => runtime::log(&alloc::format!("TRINO_RSP:oracle {what}: {m}\n")),
            Err(m) => runtime::log(&alloc::format!("TRINO_TEST_FAIL:rsp oracle {what}: {m}\n")),
        }
    }
    pixel_check(r, "gouraud", &|r| {
        render(r, Set::Grid(GRID, Shading::Gouraud))
    });
    pixel_check(r, "colored emissive", &|r| {
        r.begin_frame(Color::BLACK);
        setup(r);
        r.set_light(&WARM);
        r.draw_model_matrix(GRID, &grid, Material::VertexLit, &GLOW);
        r.end_frame();
    });
    pixel_check(r, "double-sided back", &|r| {
        r.begin_frame(Color::BLACK);
        setup(r);
        r.draw_model_matrix(GRID, &back, Material::VertexLit, &both);
        r.end_frame();
    });
    pixel_check(r, "fog", &|r| {
        r.begin_frame(Color::BLACK);
        setup(r);
        r.set_light(&FOGGY);
        r.draw_model_matrix(GRID, &turned, Material::VertexLit, &ModelParams::DEFAULT);
        r.end_frame();
    });
    pixel_check(r, "flat squashed", &|r| {
        r.begin_frame(Color::BLACK);
        setup(r);
        r.draw_model_matrix(BUMPY, &squash, Material::VertexLit, &flat);
        r.end_frame();
    });
    pixel_check(r, "textured", &|r| {
        r.begin_frame(Color::BLACK);
        setup(r);
        r.draw_model_matrix(card, &grid, Material::VertexLit, &unlit);
        r.end_frame();
    });
    pixel_check(r, "rigid parts", &|r| {
        r.begin_frame(Color::BLACK);
        setup(r);
        r.draw_model_matrix(RIGID, &grid, Material::VertexLit, &ModelParams::DEFAULT);
        r.end_frame();
    });
    setup(r);
}

/// The palette command against the CPU path (GitHub #122): oracles under the
/// white light, the warm light with a glow, a flat copy and the textured
/// overlay, then the same draws' pixels, each also proven to have taken
/// the RSP (a draw left on the CPU would pass the pixel check trivially).
/// Run last, with the meshes of `register_meshes(r, true)`.
fn palette_checks(r: &mut N64Renderer) {
    setup(r);
    let grid = translation(Vec3::new(0.0, 0.0, 4.0));
    let (squash, flat) = squashed();
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let recolor = ModelParams {
        palette: Some(CHECK_PALETTE),
        ..ModelParams::DEFAULT
    };
    let palette = r.rsp_oracle(GRID_SLOTS, &grid, &recolor);
    let flat_recolor = ModelParams {
        palette: Some(CHECK_PALETTE),
        ..flat
    };
    let flat_palette = r.rsp_oracle(BUMPY_SLOTS, &squash, &flat_recolor);
    r.set_light(&WARM);
    let warm_palette = r.rsp_oracle(
        GRID_SLOTS,
        &grid,
        &ModelParams {
            palette: Some(CHECK_PALETTE),
            ..GLOW
        },
    );
    setup(r);
    let unlit_recolor = ModelParams {
        palette: Some(CHECK_PALETTE),
        ..unlit
    };
    let textured_palette = r.rsp_oracle(CARD_SLOTS, &grid, &unlit_recolor);
    for (what, res) in [
        ("palette", palette),
        ("palette warm", warm_palette),
        ("palette flat", flat_palette),
        ("palette textured", textured_palette),
    ] {
        match res {
            Ok(m) => runtime::log(&alloc::format!("TRINO_RSP:oracle {what}: {m}\n")),
            Err(m) => runtime::log(&alloc::format!("TRINO_TEST_FAIL:rsp oracle {what}: {m}\n")),
        }
    }
    let recolored: [(&str, ModelId, &Mat34, ModelParams); 4] = [
        ("palette", GRID_SLOTS, &grid, recolor),
        ("palette flat", BUMPY_SLOTS, &squash, flat_recolor),
        ("palette rigid", RIGID_SLOTS, &grid, recolor),
        ("palette textured", CARD_SLOTS, &grid, unlit_recolor),
    ];
    for (what, model, at, params) in recolored {
        let draw = |r: &mut N64Renderer| {
            r.begin_frame(Color::BLACK);
            setup(r);
            r.draw_model_matrix(model, at, Material::VertexLit, &params);
            r.end_frame();
        };
        pixel_check(r, what, &draw);
        draw(r);
        let rsp = r.stats().tnl.rsp_draws;
        if rsp == 0 {
            runtime::log(&alloc::format!(
                "TRINO_TEST_FAIL:rsp {what}: the palette draw stayed on the CPU\n"
            ));
        } else {
            runtime::log(&alloc::format!(
                "TRINO_RSP:{what} on the RSP: {rsp} commands\n"
            ));
        }
    }
    setup(r);
}

/// The frame `draw` makes, through the CPU path and through the RSP: a
/// 12×12 sample of the grid's pixels must agree (a tenth may differ by more
/// than 24 on edges), and the RSP frame must not be empty.
fn pixel_check(r: &mut N64Renderer, what: &str, draw: &dyn Fn(&mut N64Renderer)) {
    let sample = |r: &mut N64Renderer, rsp: bool| {
        r.set_rsp(rsp);
        draw(r);
        let mut px = Vec::new();
        for y in 0..12 {
            for x in 0..12 {
                px.push(
                    r.read_pixel(118 + x * 7, 78 + y * 7)
                        .unwrap_or(Color::BLACK),
                );
            }
        }
        px
    };
    let cpu = sample(r, false);
    let rsp = sample(r, true);
    let (mut sum, mut far) = (0u32, 0u32);
    for (a, b) in cpu.iter().zip(&rsp) {
        let d = a.r.abs_diff(b.r) as u32 + a.g.abs_diff(b.g) as u32 + a.b.abs_diff(b.b) as u32;
        sum += d;
        far += (d > 24) as u32;
    }
    let mut map = String::new();
    for (i, (a, b)) in cpu.iter().zip(&rsp).enumerate() {
        let d = a.r.abs_diff(b.r) as u32 + a.g.abs_diff(b.g) as u32 + a.b.abs_diff(b.b) as u32;
        map.push(if d > 24 { '#' } else { '.' });
        if i % 12 == 11 {
            map.push('|');
        }
    }
    r.set_rsp(true);
    let msg = alloc::format!(
        "{} pixels, mean diff {:.2}/255, {far} far (cpu {:?} rsp {:?})",
        cpu.len(),
        sum as f32 / (cpu.len() * 3) as f32,
        cpu[66],
        rsp[66]
    );
    let blank = rsp.iter().all(|c| c.r | c.g | c.b == 0);
    if far > cpu.len() as u32 / 10 || blank {
        runtime::log(&alloc::format!("TRINO_RSP:pixel map {what} {map}\n"));
        runtime::log(&alloc::format!(
            "TRINO_TEST_FAIL:rsp pixels {what}: {msg}\n"
        ));
    } else {
        runtime::log(&alloc::format!("TRINO_RSP:pixels {what}: {msg}\n"));
    }
}

fn measure(r: &mut N64Renderer, name: &str, suffix: &str, set: Set, frames: u32) {
    {
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
            r.write_report(&p, &alloc::format!("{name}{suffix}"), &times, &mut report);
            runtime::log(&report);
        }
    }
}

/// The grid's middle column: the left part takes columns `0..=half`, the
/// right one `half..`.
fn n_grid_half() -> usize {
    GRID_N / 2
}

/// [`RIGID`]: the grid's two halves, each with its own copy of the middle
/// column, and a `SKEL` of two identity joints (the bind pose; no clips).
fn rigid_grid(pos: &[Vec3], half: usize, radius: f32, slotted: bool) -> Vec<u8> {
    let mut verts = Vec::new();
    let mut joint = Vec::new();
    let mut slots = Vec::new();
    let mut idx = Vec::new();
    for (j, cols) in [(0u8, 0..half + 1), (1, half..GRID_N)].into_iter() {
        let base = verts.len() as u16;
        let w = cols.len();
        for y in 0..GRID_N {
            for x in cols.clone() {
                verts.push(pos[y * GRID_N + x]);
                joint.push(j);
                slots.push(grid_slot(x, y));
            }
        }
        for y in 0..GRID_N - 1 {
            for x in 0..w - 1 {
                let i = base + (y * w + x) as u16;
                let n = w as u16;
                idx.extend_from_slice(&[i, i + n, i + 1, i + 1, i + n, i + n + 1]);
            }
        }
    }
    let mut b = tmdl(&verts, &idx, Vec3::ZERO, radius);
    let mut skel = Vec::new();
    skel.extend_from_slice(&2u32.to_le_bytes());
    skel.extend_from_slice(&joint);
    let ident = [
        1.0f32, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 0.0, 1.0, 0.0,
    ];
    let rest = [0.0f32, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0, 1.0, 1.0, 1.0];
    for parent in [-1i32, 0] {
        skel.extend_from_slice(&parent.to_le_bytes());
        for v in ident.iter().chain(&rest) {
            skel.extend_from_slice(&v.to_le_bytes());
        }
    }
    b.extend_from_slice(b"SKEL");
    b.extend_from_slice(&(skel.len() as u32).to_le_bytes());
    b.extend_from_slice(&skel);
    if slotted { with_slots(b, &slots) } else { b }
}

/// The water overlay against the CPU path (`rsp_water_oracle`): castle64's
/// self-test sheet seen from above, with a swell, a glint, a warm light and
/// fog, then a ripple ringing out of its middle.
fn water_checks(r: &mut N64Renderer) {
    use castle64::selftest::{WATER_DETAIL, WATER_SHEET, WATER_TEX};
    use trino_core::{DetailTexture, Ripple, Vec2, Water};
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 3.0, -2.2),
        target: Vec3::new(0.0, 0.0, 0.2),
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    });
    r.set_light(&Light {
        fog: Fog {
            color: Color::rgb(132, 178, 226),
            near: 2.5,
            far: 6.0,
        },
        ..WARM
    });
    let params = ModelParams {
        texture: Some(WATER_TEX),
        detail: Some(DetailTexture::new(WATER_DETAIL, 1)),
        gloss: 220,
        water: Some(Water {
            wave_amplitude: 0.08,
            wave_frequency: 0.9,
            ..Water::DEFAULT
        }),
        ..ModelParams::DEFAULT
    };
    let ripple = Ripple {
        pos: Vec2::new(0.1, 0.2),
        age: 0.4,
        strength: 0.12,
    };
    for (what, ripples) in [("swell", &[][..]), ("ripple", &[ripple][..])] {
        match r.rsp_water_oracle(WATER_SHEET, &Mat34::IDENTITY, &params, 1.3, ripples) {
            Ok(m) => runtime::log(&alloc::format!("TRINO_RSP:oracle water {what}: {m}\n")),
            Err(m) => runtime::log(&alloc::format!(
                "TRINO_TEST_FAIL:rsp oracle water {what}: {m}\n"
            )),
        }
    }
    setup(r);
}
