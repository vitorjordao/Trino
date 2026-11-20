//! Blended triangles never hide what is behind them (the CPU path's batch,
//! `batch_keys`): a translucent draw writes no depth and is drawn after the
//! batch's opaque triangles, back to front across textures.
//!
//! The bug this guards: the batch grouped triangles by texture before depth,
//! and blended ones wrote depth, so a translucent untextured box (texture
//! group 0, drawn first) hid a textured box behind it: through the glass
//! there was only the clear color.
//!
//! GPU-gated like the goldens: no adapter = skip, unless
//! `TRINO_REQUIRE_GPU` is set.

mod common;

use common::*;
use trino_core::render3d::{Camera3, Fog, Light};
use trino_core::{Color, Material, ModelParams, Renderer, Shading, Transform3, Vec3};
use trino_platform_pc::{PcRenderer, SimProfile};

fn light() -> Light {
    Light {
        dir: Vec3::new(0.0, 0.0, 1.0),
        ambient: 1.0,
        color: Color::WHITE,
        ambient_color: Color::WHITE,
        fog: Fog::NONE,
    }
}

/// The frame's center pixel after a translucent red box in front (drawn
/// first) and, when `behind`, an opaque green textured box behind it (drawn
/// after, same batch).
fn center(r: &mut PcRenderer, behind: bool) -> [u8; 4] {
    r.begin_frame(Color::BLACK);
    r.set_camera(&Camera3::default());
    r.set_light(&light());
    let glass = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::tinted(Color::rgba(255, 0, 0, 128))
    };
    r.draw_model(
        model(BOX),
        &Transform3 {
            position: Vec3::new(0.0, 0.0, -2.0),
            ..Transform3::default()
        },
        Material::VertexLit,
        &glass,
    );
    if behind {
        r.draw_model(
            model(TBOX),
            &Transform3 {
                position: Vec3::new(0.0, 0.0, 3.0),
                scale: Vec3::new(4.0, 4.0, 4.0),
                ..Transform3::default()
            },
            Material::VertexLit,
            &ModelParams {
                shading: Shading::Unlit,
                ..ModelParams::DEFAULT
            },
        );
    }
    r.end_frame();
    let (w, h) = r.internal_size();
    let c = r.read_pixel(w / 2, h / 2).expect("inside the frame");
    [c.r, c.g, c.b, c.a]
}

#[test]
fn what_is_behind_a_translucent_draw_still_shows() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);
    // Both paths: every draw on the CPU path (the batch's order alone), and
    // the GPU path's opaque meshes under the CPU path's blended triangles.
    for hw in [false, true] {
        r.set_hw_tnl(hw);
        let alone = center(&mut r, false);
        let both = center(&mut r, true);
        assert!(
            alone[0] > 90 && alone[1] < 20,
            "hw_tnl={hw}: half red over black: {alone:?}"
        );
        assert!(
            both[0] > 90 && both[1] > 90 && both[2] < 30,
            "hw_tnl={hw}: half red over the green box behind it: {both:?} \
             (the box vanished: the glass wrote depth before it was drawn)"
        );
    }
}

/// A green box whose texture is `rgba` (16×16) in front of a big opaque
/// red box: the frame's pixels.
fn masked_frame(r: &mut PcRenderer) -> Vec<u8> {
    r.begin_frame(Color::BLACK);
    r.set_camera(&Camera3::default());
    r.set_light(&light());
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    r.draw_model(
        model(BOX),
        &Transform3 {
            position: Vec3::new(0.0, 0.0, 3.0),
            scale: Vec3::new(4.0, 4.0, 4.0),
            ..Transform3::default()
        },
        Material::VertexLit,
        &ModelParams {
            tint: Color::rgb(255, 0, 0),
            ..unlit
        },
    );
    r.draw_model(
        model(TBOX),
        &Transform3 {
            position: Vec3::new(0.0, 0.0, -2.0),
            scale: Vec3::new(1.5, 1.5, 1.5),
            ..Transform3::default()
        },
        Material::VertexLit,
        &unlit,
    );
    r.end_frame();
    r.read_offscreen()
}

/// A texture whose alpha is a mask (a fence, a grille: every texel clear or
/// solid) draws on the GPU path, cut at half alpha and depth-written, and
/// shows what the CPU path's blending shows; one that is half transparent
/// stays on the CPU path. The fence of a kart track used to cost a third of
/// the frame on the CPU path.
#[test]
fn a_masked_texture_is_cut_on_the_gpu_path() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);
    // 4×4-texel cells, clear and solid in a checker.
    let mask: Vec<u8> = (0..16 * 16)
        .flat_map(|i| {
            let (x, y) = (i % 16 / 4, i / 16 / 4);
            [0, 255, 0, if (x + y) % 2 == 0 { 255 } else { 0 }]
        })
        .collect();
    r.upload_sprite(sprite(TEX), 16, 16, &mask);
    let d = trino_platform_pc::tnl_oracle::compare(&mut r, masked_frame)
        .expect("the native profile has the GPU path");
    eprintln!("masked: {d}");
    assert_eq!(d.gpu_draws, 2, "both boxes on the GPU path");
    // The cut and the blend only differ along the cells' filtered edges.
    assert!(d.mean < 4.0 && d.far_share < 0.04, "{d}");
    r.set_hw_tnl(true);
    let px = masked_frame(&mut r);
    assert_eq!(r.pc_gpu_rejects().iter().sum::<u32>(), 0);
    let count = |want: fn(&[u8]) -> bool| px.chunks(4).filter(|p| want(p)).count();
    let green = count(|p| p[1] > 200 && p[0] < 60);
    let red_through = count(|p| p[0] > 200 && p[1] < 60);
    assert!(green > 500, "the solid cells: {green}");
    assert!(
        red_through > 500,
        "the red box through the holes: {red_through}"
    );
    // Half transparent: blended back to front on the CPU path.
    let glass: Vec<u8> = (0..16 * 16).flat_map(|_| [0, 255, 0, 128]).collect();
    r.upload_sprite(sprite(TEX), 16, 16, &glass);
    masked_frame(&mut r);
    let off = r.pc_gpu_rejects();
    assert_eq!(
        off[trino_core::render3d::PcGpuReject::TextureAlpha as usize],
        1
    );
}
