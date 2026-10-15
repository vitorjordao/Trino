//! `Renderer::build_batch` / `drop_batch` on the PC backend: a batch of two
//! tinted cubes draws the same pixels as the two cubes drawn one by one,
//! unknown parts are refused, and a dropped batch draws nothing.
//! GPU-gated like the other render tests (no adapter = skip).

mod common;

use common::*;
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{BatchPart, Color, Mat34, Material, ModelId, ModelParams, Renderer, Vec3};
use trino_platform_pc::{PcRenderer, SimProfile};

fn cube() -> Vec<u8> {
    // 6 faces, 24 vertices, unit cube centred on the origin.
    let faces = [
        (Vec3::new(1.0, 0.0, 0.0), [200u8, 60, 60, 255]),
        (Vec3::new(-1.0, 0.0, 0.0), [200, 60, 60, 255]),
        (Vec3::new(0.0, 1.0, 0.0), [60, 200, 60, 255]),
        (Vec3::new(0.0, -1.0, 0.0), [60, 200, 60, 255]),
        (Vec3::new(0.0, 0.0, 1.0), [60, 60, 200, 255]),
        (Vec3::new(0.0, 0.0, -1.0), [60, 60, 200, 255]),
    ];
    let (mut pos, mut nrm, mut col, mut idx) = (vec![], vec![], vec![], vec![]);
    for (f, (n, c)) in faces.iter().enumerate() {
        let u = if n.y.abs() > 0.9 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(0.0, 1.0, 0.0).cross(*n).normalized()
        };
        let v = n.cross(u);
        let base = (f * 4) as u16;
        for (su, sv) in [(-0.5f32, -0.5f32), (0.5, -0.5), (0.5, 0.5), (-0.5, 0.5)] {
            pos.push(*n * 0.5 + u * su + v * sv);
            nrm.push(*n);
            col.push(*c);
        }
        idx.extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let mut b = b"TMDL".to_vec();
    b.extend_from_slice(&(pos.len() as u32).to_le_bytes());
    b.extend_from_slice(&(idx.len() as u32).to_le_bytes());
    for p in &pos {
        for x in [p.x, p.y, p.z] {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    for p in &nrm {
        for x in [p.x, p.y, p.z] {
            b.extend_from_slice(&x.to_le_bytes());
        }
    }
    for c in &col {
        b.extend_from_slice(c);
    }
    for i in &idx {
        b.extend_from_slice(&i.to_le_bytes());
    }
    b
}

fn frame(r: &mut PcRenderer, draw: impl FnOnce(&mut PcRenderer)) -> Vec<u8> {
    r.begin_frame(Color::BLACK);
    r.set_camera(&Camera3 {
        eye: Vec3::new(3.0, 3.0, 5.0),
        target: Vec3::ZERO,
        fov_y: 0.9,
    });
    r.set_light(&DEFAULT_LIGHT);
    draw(r);
    r.end_frame();
    r.read_offscreen()
}

#[test]
fn a_batch_draws_like_its_parts_and_drops_cleanly() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let cube_id = ModelId::from_path("models/test_cube");
    r.upload_mesh(cube_id, cube());
    let at = |x: f32| {
        Mat34::from_rotation_scale_translation(
            Vec3::new(0.0, 0.5, 0.0),
            Vec3::ONE,
            Vec3::new(x, 0.0, 0.0),
        )
    };
    let tints = [Color::WHITE, Color::rgb(255, 160, 80)];
    let one_by_one = frame(&mut r, |r| {
        for (k, x) in [-1.0f32, 1.0].iter().enumerate() {
            r.draw_model_matrix(
                cube_id,
                &at(*x),
                Material::VertexLit,
                &ModelParams::tinted(tints[k]),
            );
        }
    });
    let batch = ModelId(0x7B00_0001);
    let parts = [
        BatchPart {
            model: cube_id,
            matrix: at(-1.0),
            tint: tints[0],
        },
        BatchPart {
            model: cube_id,
            matrix: at(1.0),
            tint: tints[1],
        },
    ];
    assert_eq!(r.build_batch(batch, &parts), 2, "the PC takes every part");
    let batched = frame(&mut r, |r| {
        r.draw_model_matrix(
            batch,
            &Mat34::IDENTITY,
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
    });
    let lit = one_by_one.chunks(4).filter(|p| p[..3] != [0, 0, 0]).count();
    assert!(lit > 1000, "the cubes are on screen ({lit} px)");
    let differ = one_by_one
        .chunks(4)
        .zip(batched.chunks(4))
        .filter(|(a, b)| a.iter().zip(b.iter()).any(|(x, y)| x.abs_diff(*y) > 2))
        .count();
    assert!(differ * 100 < lit, "{differ} of {lit} pixels differ");
    // A part the renderer does not know: nothing merged.
    let unknown = [BatchPart {
        model: ModelId::from_path("models/nope"),
        matrix: Mat34::IDENTITY,
        tint: Color::WHITE,
    }];
    assert_eq!(r.build_batch(ModelId(0x7B00_0002), &unknown), 0);
    assert_eq!(r.build_batch(ModelId(0x7B00_0003), &[]), 0);
    // Dropped: drawing the id draws nothing.
    r.drop_batch(batch);
    let gone = frame(&mut r, |r| {
        r.draw_model_matrix(
            batch,
            &Mat34::IDENTITY,
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
    });
    assert!(gone.chunks(4).all(|p| p[..3] == [0, 0, 0]));
}
