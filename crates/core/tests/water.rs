//! Water surfaces (ADR 0015) through the public T&L API: a water draw is
//! displaced by the swell and the ripples, its UVs flow, its colors carry
//! the Fresnel sky; the per-vertex tiers get a ring per ripple; the per-pixel
//! tier's surface path hands over the displaced normals; draws without
//! water are bit-identical whatever the frame's water state; water never
//! takes the N64's RSP, the 3DS's GPU T&L or the draw memo.

mod common;

use common::*;
use trino_core::memo::MemoKey;
use trino_core::render3d::{
    self, Camera3, DEFAULT_LIGHT, Mesh, RING_SEGMENTS, ScreenTri, SurfaceScratch, SurfaceVert,
    TnlContext, TnlScratch, TnlStats,
};
use trino_core::{
    Mat34, ModelParams, Ripple, RspReject, Shading, SpriteId, Vec2, Vec3, Water, WaterState,
};

/// A `2 half` wide square sheet at y 0, `n` x `n` quads, normals up, UVs one
/// repeat per unit, translucent blue, with its bounds.
fn sheet(n: u16, half: f32) -> Vec<u8> {
    let mut pos = Vec::new();
    let mut uv = Vec::new();
    for j in 0..=n {
        for i in 0..=n {
            let x = -half + 2.0 * half * i as f32 / n as f32;
            let z = -half + 2.0 * half * j as f32 / n as f32;
            pos.push(Vec3::new(x, 0.0, z));
            uv.extend_from_slice(&[x, z]);
        }
    }
    let mut idx = Vec::new();
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            // Front-facing seen from above.
            idx.extend_from_slice(&[a, a + n + 2, a + 1, a, a + n + 1, a + n + 2]);
        }
    }
    let count = pos.len();
    let mut blob = tmdl(
        &pos,
        &vec![Vec3::new(0.0, 1.0, 0.0); count],
        &vec![[40, 110, 230, 160]; count],
        &idx,
    );
    let mut uvs = Vec::new();
    f32s(&mut uvs, &uv);
    section(&mut blob, b"TXUV", &uvs);
    section(&mut blob, b"TXID", &SpriteId(7).0.to_le_bytes());
    section(&mut blob, b"BNDS", &bnds(Vec3::ZERO, half * 1.5));
    blob
}

fn camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 4.0, -6.0),
        target: Vec3::ZERO,
        ..Camera3::default()
    }
}

fn ctx(state: &WaterState) -> TnlContext {
    TnlContext::new(&camera(), &DEFAULT_LIGHT, SCREEN).with_water(state)
}

fn draw(ctx: &TnlContext, blob: &[u8], params: &ModelParams) -> (Vec<ScreenTri>, TnlStats) {
    let mesh = Mesh::from_tmdl(blob).unwrap();
    let mut out = Vec::new();
    let mut scratch = TnlScratch::new();
    let s = render3d::tessellate_with(
        ctx,
        &mut scratch,
        &mesh,
        &Mat34::IDENTITY,
        params,
        &mut |t| out.push(t),
    );
    (out, s)
}

fn water(w: Water) -> ModelParams {
    ModelParams {
        water: Some(w),
        ..ModelParams::DEFAULT
    }
}

const CALM: Water = Water {
    flow_speed: Vec2::new(0.0, 0.5),
    detail_flow: Vec2::new(0.25, -0.5),
    wave_amplitude: 0.0,
    wave_frequency: 0.5,
    ripple_responsiveness: 1.0,
};

/// The screen triangles' corners (sorted, so order doesn't matter).
fn corners(tris: &[ScreenTri]) -> Vec<(i32, i32)> {
    let mut v: Vec<(i32, i32)> = tris
        .iter()
        .flat_map(|t| {
            t.pts
                .iter()
                .map(|p| ((p.x * 16.0) as i32, (p.y * 16.0) as i32))
        })
        .collect();
    v.sort();
    v
}

#[test]
fn draws_without_water_are_unchanged_by_the_water_state() {
    let blob = sheet(4, 2.0);
    let ripple = Ripple {
        pos: Vec2::new(0.5, 0.5),
        age: 0.3,
        strength: 0.2,
    };
    let state = WaterState::new(3.7, &[ripple], 4);
    let plain = ModelParams::DEFAULT;
    let (a, sa) = draw(&ctx(&WaterState::NONE), &blob, &plain);
    let (b, sb) = draw(&ctx(&state), &blob, &plain);
    assert_eq!(sa, sb);
    assert!(
        a.iter()
            .zip(&b)
            .all(|(x, y)| { x.pts == y.pts && x.colors == y.colors && x.z == y.z && x.uv == y.uv })
    );
}

#[test]
fn the_swell_moves_the_surface_and_its_time_moves_the_swell() {
    let blob = sheet(8, 2.0);
    let calm = draw(&ctx(&WaterState::NONE), &blob, &water(CALM)).0;
    let swell = Water {
        wave_amplitude: 0.3,
        ..CALM
    };
    let at0 = draw(&ctx(&WaterState::new(0.0, &[], 4)), &blob, &water(swell)).0;
    let at1 = draw(&ctx(&WaterState::new(0.7, &[], 4)), &blob, &water(swell)).0;
    assert!(
        at0.iter().all(|t| t.depth > 0.0),
        "no rings without ripples"
    );
    assert_ne!(corners(&calm), corners(&at0), "the swell displaced it");
    assert_ne!(corners(&at0), corners(&at1), "and it moves with time");
    // A calm water surface lies where the plain mesh does (to rounding).
    let plain = draw(&ctx(&WaterState::NONE), &blob, &ModelParams::DEFAULT).0;
    for (a, b) in corners(&plain).iter().zip(corners(&calm).iter()) {
        assert!(
            (a.0 - b.0).abs() <= 1 && (a.1 - b.1).abs() <= 1,
            "{a:?} {b:?}"
        );
    }
}

#[test]
fn the_texture_flows_and_grazing_angles_turn_to_sky() {
    let blob = sheet(4, 2.0);
    let p = ModelParams {
        shading: Shading::Unlit,
        ..water(CALM)
    };
    let t0 = draw(&ctx(&WaterState::new(0.0, &[], 4)), &blob, &p).0;
    let t1 = draw(&ctx(&WaterState::new(0.5, &[], 4)), &blob, &p).0;
    // Flow (0, 0.5) for 0.5 s: every UV a quarter repeat on in v.
    for (a, b) in t0.iter().zip(&t1) {
        for k in 0..3 {
            let d = b.uv[k] - a.uv[k];
            assert!(d.x.abs() < 1e-4 && (d.y - 0.25).abs() < 1e-4, "{d:?}");
        }
    }
    // Unlit water is still more opaque (Fresnel) than its vertex alpha.
    assert!(t0.iter().all(|t| t.colors.iter().all(|c| c.a >= 160)));
    let plain = draw(
        &ctx(&WaterState::NONE),
        &blob,
        &ModelParams {
            shading: Shading::Unlit,
            ..ModelParams::DEFAULT
        },
    )
    .0;
    assert!(plain.iter().all(|t| t.colors.iter().all(|c| c.a == 160)));
}

#[test]
fn each_ripple_that_reaches_the_draw_adds_a_ring_drawn_last() {
    let blob = sheet(8, 2.0);
    let near = Ripple {
        pos: Vec2::new(0.0, 0.0),
        age: 0.4,
        strength: 0.1,
    };
    let far = Ripple {
        pos: Vec2::new(30.0, 0.0),
        ..near
    };
    let (none, s0) = draw(&ctx(&WaterState::new(0.0, &[], 4)), &blob, &water(CALM));
    let (one, s1) = draw(&ctx(&WaterState::new(0.0, &[near], 4)), &blob, &water(CALM));
    let (two, _) = draw(
        &ctx(&WaterState::new(0.0, &[near, far], 4)),
        &blob,
        &water(CALM),
    );
    let rings = one.iter().filter(|t| t.depth == 0.0).count();
    assert!(
        rings > 0 && rings <= 2 * RING_SEGMENTS,
        "{rings} ring triangles"
    );
    assert_eq!(s1.tris_in - s0.tris_in, 2 * RING_SEGMENTS as u32);
    assert_eq!(two.len(), one.len(), "a ripple out of reach adds nothing");
    assert!(none.iter().all(|t| t.depth > 0.0));
    // The ripple also moves the surface around it.
    let surface = |v: &[ScreenTri]| {
        corners(
            &v.iter()
                .filter(|t| t.depth > 0.0)
                .copied()
                .collect::<Vec<_>>(),
        )
    };
    assert_ne!(surface(&none), surface(&one));
    // No response, no ripple.
    let deaf = Water {
        ripple_responsiveness: 0.0,
        ..CALM
    };
    let (quiet, _) = draw(&ctx(&WaterState::new(0.0, &[near], 4)), &blob, &water(deaf));
    assert_eq!(quiet.len(), none.len());
}

#[test]
fn the_surface_path_hands_over_the_displaced_normals() {
    let blob = sheet(8, 2.0);
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let swell = Water {
        wave_amplitude: 0.3,
        ..CALM
    };
    let mut scratch = SurfaceScratch::new();
    let mut verts: Vec<SurfaceVert> = Vec::new();
    let mut n = 0;
    render3d::tessellate_surface(
        &ctx(&WaterState::new(0.3, &[], 4)),
        &mut scratch,
        &mesh,
        &Mat34::IDENTITY,
        &water(swell),
        &mut |_, v| {
            n += 1;
            verts.extend_from_slice(v);
        },
    );
    assert!(n > 0);
    let tilted = verts.iter().filter(|v| v.normal.y < 0.99).count();
    assert!(tilted > verts.len() / 2, "{tilted} of {}", verts.len());
    assert!(
        verts.iter().any(|v| v.pos.y.abs() > 0.05),
        "displaced positions"
    );
    // The per-pixel tier draws the rings itself: none here.
    let ripple = Ripple {
        pos: Vec2::ZERO,
        age: 0.4,
        strength: 0.1,
    };
    let mut with = 0;
    render3d::tessellate_surface(
        &ctx(&WaterState::new(0.3, &[ripple], 4)),
        &mut scratch,
        &mesh,
        &Mat34::IDENTITY,
        &water(swell),
        &mut |_, _| with += 1,
    );
    assert_eq!(with, n);
}

#[test]
fn water_takes_the_water_overlay_and_stays_off_the_gpu_path_and_the_memo() {
    let blob = sheet(2, 1.0);
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let ctx = ctx(&WaterState::NONE);
    let p = ModelParams {
        shading: Shading::Unlit,
        texture: None,
        ..water(CALM)
    };
    // The N64's water overlay takes a textured sheet the CPU would neither
    // clip nor split, lit in world space ...
    let d = render3d::rsp_check(&ctx, &mesh, &Mat34::IDENTITY, &p).unwrap();
    assert_eq!(d.ambient, 1.0, "unlit");
    assert_eq!(
        render3d::rsp_plan(&ctx, &mesh, &Mat34::IDENTITY, &p),
        Ok((1, 8))
    );
    // ... not one the CPU would clip or split (a sheet 40 units wide, 20 a
    // quad, under the camera) ...
    let wide = sheet(2, 20.0);
    let wide_m = Mesh::from_tmdl(&wide).unwrap();
    assert!(render3d::rsp_check(&ctx, &wide_m, &Mat34::IDENTITY, &p).is_err());
    // ... nor an untextured one.
    assert_eq!(
        render3d::rsp_water_mesh_check(
            &Mesh::from_tmdl(&tmdl(
                &[
                    Vec3::ZERO,
                    Vec3::new(0.0, 0.0, 1.0),
                    Vec3::new(1.0, 0.0, 0.0)
                ],
                &[Vec3::new(0.0, 1.0, 0.0); 3],
                &[[40, 110, 230, 160]; 3],
                &[0, 1, 2],
            ))
            .unwrap()
        ),
        Err(RspReject::Water)
    );
    assert_eq!(
        render3d::gpu_check(&ctx, &mesh, &Mat34::IDENTITY, &p).unwrap_err(),
        render3d::GpuReject::Water
    );
    assert!(MemoKey::new(1, 0, &Mat34::IDENTITY, &p).is_none());
    assert!(RspReject::Water.label().contains("water"));
}
