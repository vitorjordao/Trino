//! Static batching (`trino_core::batch`) end to end: parts → merged TMDL →
//! `Mesh` → `tessellate`, compared with drawing the parts one by one. A
//! batch must look exactly like its parts (positions, lighting under
//! rotation and non-uniform scale, tints), cull as one, and refuse what it
//! cannot hold.

mod common;

use common::*;
use trino_core::batch::{
    BatchSource, batchable, merge_parts, merge_tmdl, merged_size, merged_size_of,
};
use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, ScreenTri, rsp_mesh_supported, tessellate,
};
use trino_core::{BatchPart, Color, Mat34, ModelId, ModelParams, Vec2, Vec3};

const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

fn camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(6.0, 7.0, 9.0),
        target: Vec3::new(0.0, 0.0, 0.0),
        fov_y: 0.8,
    }
}

fn light() -> Light {
    Light {
        dir: Vec3::new(-0.35, -0.85, -0.4),
        ambient: 0.5,
        ..DEFAULT_LIGHT
    }
}

fn draw(mesh: &Mesh, m: &Mat34, params: &ModelParams) -> Vec<ScreenTri> {
    let mut out = Vec::new();
    tessellate(mesh, m, &camera(), &light(), params, SCREEN, &mut |t| {
        out.push(t)
    });
    out
}

fn sort(mut v: Vec<ScreenTri>) -> Vec<ScreenTri> {
    v.sort_by(|a, b| {
        let k = |t: &ScreenTri| (t.pts[0].x, t.pts[0].y, t.pts[1].x, t.pts[1].y);
        k(a).partial_cmp(&k(b)).unwrap()
    });
    v
}

fn close(a: &[ScreenTri], b: &[ScreenTri]) {
    assert_eq!(a.len(), b.len(), "triangle count");
    for (x, y) in a.iter().zip(b) {
        for k in 0..3 {
            assert!(
                (x.pts[k].x - y.pts[k].x).abs() < 0.01,
                "{:?} vs {:?}",
                x.pts,
                y.pts
            );
            assert!((x.pts[k].y - y.pts[k].y).abs() < 0.01);
            for (c, d) in [
                (x.colors[k].r, y.colors[k].r),
                (x.colors[k].g, y.colors[k].g),
                (x.colors[k].b, y.colors[k].b),
            ] {
                assert!((c as i32 - d as i32).abs() <= 1, "color {c} vs {d}");
            }
        }
    }
}

fn parts() -> Vec<(Mat34, Color)> {
    vec![
        (
            Mat34::from_rotation_scale_translation(
                Vec3::ZERO,
                Vec3::ONE,
                Vec3::new(-1.5, 0.5, 0.0),
            ),
            Color::WHITE,
        ),
        // A quarter turn with a tint.
        (
            Mat34::from_rotation_scale_translation(
                Vec3::new(0.0, core::f32::consts::FRAC_PI_2, 0.0),
                Vec3::ONE,
                Vec3::new(1.5, 0.5, 0.0),
            ),
            Color::rgb(255, 120, 60),
        ),
        // Non-uniform scale (a stretched wall): normals need the inverse transpose.
        (
            Mat34::from_rotation_scale_translation(
                Vec3::new(0.0, 0.4, 0.0),
                Vec3::new(3.0, 0.5, 1.0),
                Vec3::new(0.0, 0.25, -2.0),
            ),
            Color::rgb(90, 200, 255),
        ),
    ]
}

#[test]
fn a_batch_draws_exactly_like_its_parts() {
    let cube = cube_tmdl();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let ps = parts();
    let mut one_by_one = Vec::new();
    for (m, tint) in &ps {
        one_by_one.extend(draw(&mesh, m, &ModelParams::tinted(*tint)));
    }
    let src: Vec<BatchSource> = ps
        .iter()
        .map(|(m, t)| BatchSource {
            mesh: &mesh,
            matrix: *m,
            tint: *t,
        })
        .collect();
    let mut blob = vec![0u8; merged_size(&src).unwrap()];
    let n = merge_tmdl(&src, &mut blob).unwrap();
    assert_eq!(n, blob.len());
    let merged = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(merged.vertex_count, 3 * mesh.vertex_count);
    assert_eq!(merged.index_count, 3 * mesh.index_count);
    let batched = draw(&merged, &Mat34::IDENTITY, &ModelParams::DEFAULT);
    close(&sort(batched), &sort(one_by_one));
}

#[test]
fn the_batch_bounds_cover_every_part_and_cull_as_one() {
    let cube = cube_tmdl();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let src: Vec<BatchSource> = parts()
        .iter()
        .map(|(m, t)| BatchSource {
            mesh: &mesh,
            matrix: *m,
            tint: *t,
        })
        .collect();
    let mut blob = vec![0u8; merged_size(&src).unwrap()];
    merge_tmdl(&src, &mut blob).unwrap();
    let merged = Mesh::from_tmdl(&blob).unwrap();
    let (c, r) = merged.bounds().expect("BNDS");
    for k in 0..merged.vertex_count {
        let p = merged.position(k);
        let d = Vec3::new(p.x - c.x, p.y - c.y, p.z - c.z);
        assert!(d.dot(d).sqrt() <= r + 1e-4, "vertex {k} outside the sphere");
    }
    // Far behind the camera: the whole batch is culled, nothing emitted.
    let away =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(60.0, 0.0, 90.0));
    let stats = tessellate(
        &merged,
        &away,
        &camera(),
        &light(),
        &ModelParams::DEFAULT,
        SCREEN,
        &mut |_| panic!("a culled batch emitted a triangle"),
    );
    assert_eq!(stats.culled, 1);
}

#[test]
fn merged_normals_stay_unit_so_the_rsp_path_accepts_the_batch() {
    let cube = cube_tmdl();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let src: Vec<BatchSource> = parts()
        .iter()
        .map(|(m, t)| BatchSource {
            mesh: &mesh,
            matrix: *m,
            tint: *t,
        })
        .collect();
    let mut blob = vec![0u8; merged_size(&src).unwrap()];
    merge_tmdl(&src, &mut blob).unwrap();
    let merged = Mesh::from_tmdl(&blob).unwrap();
    for k in 0..merged.vertex_count {
        let n = merged.normal(k);
        assert!((n.dot(n) - 1.0).abs() < 1e-3);
    }
    assert!(rsp_mesh_supported(&merged), "3 cubes fit one RSP draw");
}

#[test]
fn merge_parts_matches_merge_tmdl() {
    let cube = cube_tmdl();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let ps = parts();
    let meshes: Vec<Mesh> = ps.iter().map(|_| Mesh::from_tmdl(&cube).unwrap()).collect();
    let bparts: Vec<BatchPart> = ps
        .iter()
        .map(|(m, t)| BatchPart {
            model: ModelId::from_path("models/cube"),
            matrix: *m,
            tint: *t,
        })
        .collect();
    let mut a = vec![0u8; merged_size_of(&meshes).unwrap()];
    merge_parts(&meshes, &bparts, &mut a).unwrap();
    let src: Vec<BatchSource> = ps
        .iter()
        .map(|(m, t)| BatchSource {
            mesh: &mesh,
            matrix: *m,
            tint: *t,
        })
        .collect();
    let mut b = vec![0u8; merged_size(&src).unwrap()];
    merge_tmdl(&src, &mut b).unwrap();
    assert_eq!(a, b);
    // Mismatched lists are refused.
    assert!(merge_parts(&meshes[..1], &bparts, &mut a).is_err());
}

#[test]
fn what_a_batch_cannot_hold_is_refused() {
    let cube = cube_tmdl();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    // A short buffer.
    let src = [BatchSource {
        mesh: &mesh,
        matrix: Mat34::IDENTITY,
        tint: Color::WHITE,
    }];
    let mut small = vec![0u8; 10];
    assert!(merge_tmdl(&src, &mut small).is_err());
    // A skinned mesh (a rigid skeleton) cannot be merged.
    let mut skinned = cube_tmdl();
    let jov = vec![0u8; mesh.vertex_count];
    section(&mut skinned, b"SKEL", &skel(&jov, &[(-1, IDENT34, REST)]));
    let sk = Mesh::from_tmdl(&skinned).unwrap();
    assert!(!batchable(&sk));
    let src = [BatchSource {
        mesh: &sk,
        matrix: Mat34::IDENTITY,
        tint: Color::WHITE,
    }];
    let mut out = vec![0u8; 4096];
    assert!(merge_tmdl(&src, &mut out).is_err());
    // More vertices than u16 indices address.
    let many: Vec<BatchSource> = (0..2731)
        .map(|_| BatchSource {
            mesh: &mesh,
            matrix: Mat34::IDENTITY,
            tint: Color::WHITE,
        })
        .collect();
    assert!(merged_size(&many).is_none());
}

#[test]
fn an_empty_batch_is_valid_and_draws_nothing() {
    let mut blob = vec![0u8; merged_size(&[]).unwrap()];
    merge_tmdl(&[], &mut blob).unwrap();
    let m = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(m.vertex_count, 0);
    assert!(draw(&m, &Mat34::IDENTITY, &ModelParams::DEFAULT).is_empty());
}
