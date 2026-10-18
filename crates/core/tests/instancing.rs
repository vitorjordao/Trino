//! Instancing (`Renderer::draw_model_instances`, issue #51) through the
//! public API: the trait's default loop is one `draw_model_matrix` per copy
//! with `Instance::params` (same screen triangles, same work counters as
//! separate draws), and the PC GPU T&L rules (`pc_gpu_check`) decide the
//! draws the way the module documents.

mod common;

use common::*;
use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Mesh, PcGpuReject, TnlContext, pc_gpu_check, pc_gpu_mesh_check,
};
use trino_core::{
    Color, Instance, Mat34, Material, ModelId, ModelParams, Renderer, Shading, Transform3, Vec2,
    Vec3, water::Water,
};

const CUBE: ModelId = ModelId::from_path("models/cube");

fn at(x: f32) -> Mat34 {
    Transform3 {
        position: Vec3::new(x, 0.0, 0.0),
        ..Transform3::default()
    }
    .matrix()
}

fn recorder() -> Recorder {
    let mut r = Recorder::new();
    r.add_mesh(CUBE, bounded_cube());
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 1.0, -6.0),
        target: Vec3::ZERO,
        ..Camera3::default()
    });
    r
}

#[test]
fn instance_params_multiply_the_tint_and_pick_the_palette() {
    let call = ModelParams {
        tint: Color::rgba(200, 100, 255, 255),
        palette: Some([Color::rgb(1, 2, 3); 4]),
        shading: Shading::Flat,
        ..ModelParams::DEFAULT
    };
    let table = [[Color::rgb(9, 9, 9); 4], [Color::rgb(7, 7, 7); 4]];
    // White keeps the call's tint exactly; the rest of the surface is the
    // call's.
    let plain = Instance::new(Mat34::IDENTITY).params(&call, &table);
    assert_eq!(plain, call);
    let half = Instance {
        tint: Color::rgba(128, 255, 0, 128),
        palette_index: 2,
        ..Instance::new(Mat34::IDENTITY)
    }
    .params(&call, &table);
    assert_eq!(half.tint, Color::rgba(100, 100, 0, 128));
    assert_eq!(half.palette, Some(table[1]));
    assert_eq!(half.shading, Shading::Flat);
    // Past the table, or with no table: the call's palette.
    let past = Instance {
        palette_index: 3,
        ..Instance::new(Mat34::IDENTITY)
    };
    assert_eq!(past.params(&call, &table).palette, call.palette);
    assert_eq!(past.params(&call, &[]).palette, call.palette);
}

#[test]
fn the_default_loop_draws_what_separate_draws_draw() {
    let params = ModelParams::tinted(Color::rgb(255, 200, 120));
    let table = [[Color::rgb(200, 0, 0); 4]];
    let instances = [
        Instance::new(at(-2.0)),
        Instance {
            tint: Color::rgb(80, 255, 80),
            palette_index: 1,
            ..Instance::new(at(0.0))
        },
        // Far off to the side: culled, counted like a separate draw.
        Instance::new(at(400.0)),
        Instance {
            tint: Color::rgba(255, 255, 255, 100),
            ..Instance::new(at(2.0))
        },
    ];
    let mut separate = recorder();
    separate.begin_frame(Color::BLACK);
    for i in &instances {
        separate.draw_model_matrix(
            CUBE,
            &i.matrix,
            Material::VertexLit,
            &i.params(&params, &table),
        );
    }
    separate.end_frame();
    let mut one_call = recorder();
    one_call.begin_frame(Color::BLACK);
    one_call.draw_model_instances_with(CUBE, &instances, &table, &params);
    one_call.end_frame();
    assert_eq!(one_call.models(), separate.models());
    assert_eq!(
        format!("{:?}", one_call.tris),
        format!("{:?}", separate.tris),
        "bit-identical screen triangles"
    );
    assert_eq!(one_call.stats(), separate.stats());
    let s = one_call.stats();
    assert_eq!(s.models, 4);
    assert_eq!(s.tnl.culled, 1);
    assert_eq!(s.tnl.instanced, 0, "the default loop counts nothing");
    // Without a table, every copy keeps the call's palette.
    let mut plain = recorder();
    plain.begin_frame(Color::BLACK);
    plain.draw_model_instances(CUBE, &instances, &params);
    plain.end_frame();
    let models = plain.models();
    assert_eq!(models[1].2.palette, None);
    assert_eq!(models[1].2.tint, Color::rgb(80, 200, 38));
    // An empty call draws nothing.
    let mut none = recorder();
    none.begin_frame(Color::BLACK);
    none.draw_model_instances(CUBE, &[], &params);
    none.end_frame();
    assert_eq!(none.stats().models, 0);
}

#[test]
fn pc_gpu_rules_reject_only_what_the_gpu_path_does_not_port() {
    let camera = Camera3 {
        eye: Vec3::new(0.0, 1.0, -6.0),
        target: Vec3::ZERO,
        ..Camera3::default()
    };
    let ctx = TnlContext::new(&camera, &DEFAULT_LIGHT, Vec2::new(640.0, 480.0));
    let blob = bounded_cube();
    let cube = Mesh::from_tmdl(&blob).unwrap();
    assert_eq!(pc_gpu_mesh_check(&cube), Ok(()));
    let check = |m: &Mat34, p: &ModelParams| pc_gpu_check(&ctx, &cube, m, p);
    // Everything the CPU path draws opaque: flat, unlit, fog, emissive,
    // colored light, double-sided, a palette, crossing the near plane.
    let any = ModelParams {
        shading: Shading::Flat,
        emissive: Color::rgb(40, 0, 0),
        double_sided: true,
        palette: Some([Color::rgb(9, 9, 9); 4]),
        ..ModelParams::DEFAULT
    };
    assert_eq!(check(&at(0.0), &any), Ok(()));
    let near = Transform3 {
        position: Vec3::new(0.0, 1.0, -5.9),
        ..Transform3::default()
    }
    .matrix();
    assert_eq!(check(&near, &ModelParams::DEFAULT), Ok(()));
    // Blending, water, and what the CPU path culls.
    assert_eq!(
        check(
            &at(0.0),
            &ModelParams::tinted(Color::rgba(255, 255, 255, 254))
        ),
        Err(PcGpuReject::Translucent)
    );
    let water = ModelParams {
        water: Some(Water::DEFAULT),
        ..ModelParams::DEFAULT
    };
    assert_eq!(check(&at(0.0), &water), Err(PcGpuReject::Water));
    assert_eq!(
        check(&at(400.0), &ModelParams::DEFAULT),
        Err(PcGpuReject::Offscreen)
    );
    let behind = Transform3 {
        position: Vec3::new(0.0, 1.0, -20.0),
        ..Transform3::default()
    }
    .matrix();
    assert_eq!(
        check(&behind, &ModelParams::DEFAULT),
        Err(PcGpuReject::Offscreen)
    );
    // Offscreen is exactly the CPU path's cull.
    for x in [-60.0, -12.0, -6.0, 0.0, 6.0, 12.0, 60.0] {
        let m = at(x);
        let mut scratch = trino_core::TnlScratch::new();
        let s = trino_core::tessellate_with(
            &ctx,
            &mut scratch,
            &cube,
            &m,
            &ModelParams::DEFAULT,
            &mut |_| {},
        );
        assert_eq!(
            check(&m, &ModelParams::DEFAULT) == Err(PcGpuReject::Offscreen),
            s.culled == 1,
            "x = {x}"
        );
    }
    // Skeletons and smooth skinning take the GPU path since Phase 3; a
    // flat-shaded skeleton doesn't (no face normal in a vertex stage).
    let rigged = rigged_cube();
    let rigged = Mesh::from_tmdl(&rigged).unwrap();
    assert_eq!(pc_gpu_mesh_check(&rigged), Ok(()));
    let ahead = at(0.0);
    assert_eq!(
        pc_gpu_check(&ctx, &rigged, &ahead, &ModelParams::DEFAULT),
        Ok(())
    );
    let flat = ModelParams {
        shading: trino_core::Shading::Flat,
        ..ModelParams::DEFAULT
    };
    assert_eq!(
        pc_gpu_check(&ctx, &rigged, &ahead, &flat),
        Err(PcGpuReject::FlatSkinned)
    );
    assert_eq!(check(&ahead, &flat), Ok(()));
    // Every reason has a label and a key, in index order.
    for (i, r) in PcGpuReject::ALL.iter().enumerate() {
        assert_eq!(r.index(), i);
        assert!(!r.label().is_empty() && !r.key().contains(' '));
    }
}
