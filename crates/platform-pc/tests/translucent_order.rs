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
