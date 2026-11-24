//! `ModelParams::blend` (Bongo-Kart #179, ADR 0043 §2): an additive draw
//! never takes a hardware T&L path (the N64's RSP, the 3DS's PICA200, the
//! PC's GPU T&L) and is never memoized, so every backend draws it through
//! the CPU T&L's triangles, where its blend is a pipeline or blender mode.
//! Water ignores it.

use trino_core::memo::MemoKey;
use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, GpuReject, Mesh, PcGpuReject, RspReject, TnlContext, gpu_check,
    pc_gpu_check, rsp_check,
};
use trino_core::{Blend, Color, DynVertex, DynamicMesh, Mat34, ModelParams, Vec2, Vec3};

fn quad() -> Vec<u8> {
    let n = Vec3::new(0.0, 0.0, -1.0);
    let c = Color::rgb(220, 120, 30);
    let v = [
        DynVertex::new(Vec3::new(-0.5, -0.5, 0.0), n, c),
        DynVertex::new(Vec3::new(0.5, -0.5, 0.0), n, c),
        DynVertex::new(Vec3::new(0.5, 0.5, 0.0), n, c),
        DynVertex::new(Vec3::new(-0.5, 0.5, 0.0), n, c),
    ];
    let i = [0u16, 1, 2, 0, 2, 3];
    let mesh = DynamicMesh::new(&v, &i);
    let mut b = vec![0; mesh.tmdl_len().unwrap()];
    mesh.write_tmdl(&mut b).unwrap();
    b
}

fn ctx() -> TnlContext {
    let cam = Camera3 {
        eye: Vec3::new(0.0, 0.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    };
    TnlContext::new(&cam, &DEFAULT_LIGHT, Vec2::new(320.0, 240.0))
}

#[test]
fn additive_draws_stay_on_the_cpu_and_out_of_the_memo() {
    let blob = quad();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let ctx = ctx();
    let m = Mat34::IDENTITY;
    let plain = ModelParams::DEFAULT;
    let add = ModelParams {
        blend: Blend::Add,
        ..ModelParams::DEFAULT
    };
    assert_eq!(ModelParams::DEFAULT.blend, Blend::Alpha);
    assert!(rsp_check(&ctx, &mesh, &m, &plain).is_ok(), "the plain quad");
    assert_eq!(
        rsp_check(&ctx, &mesh, &m, &add).err(),
        Some(RspReject::Translucent)
    );
    assert!(gpu_check(&ctx, &mesh, &m, &plain).is_ok());
    assert_eq!(
        gpu_check(&ctx, &mesh, &m, &add).err(),
        Some(GpuReject::Translucent)
    );
    assert_eq!(pc_gpu_check(&ctx, &mesh, &m, &plain), Ok(()));
    assert_eq!(
        pc_gpu_check(&ctx, &mesh, &m, &add),
        Err(PcGpuReject::Additive)
    );
    assert_eq!(PcGpuReject::Additive.key(), "additive");
    assert!(MemoKey::new(1, 0, &m, &plain).is_some());
    assert!(MemoKey::new(1, 0, &m, &add).is_none());
    // Water blends its own way: the GPU path keeps it.
    let water = ModelParams {
        water: Some(trino_core::water::Water::DEFAULT),
        ..add
    };
    assert_eq!(pc_gpu_check(&ctx, &mesh, &m, &water), Ok(()));
}
