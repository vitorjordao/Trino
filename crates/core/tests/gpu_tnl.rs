//! The GPU T&L rules (`render3d::gpu_check`, `gpu_screen_matrix`) against
//! the CPU path they stand in for: for every draw the GPU path accepts,
//! evaluating its constants the way the 3DS's vertex shader does (in
//! floats) lands every vertex where `tessellate` puts it, with its depth and
//! its color, and the draws it refuses are the ones that need the CPU.

mod common;

use common::*;
use trino_core::render3d::{
    self, Camera3, DEFAULT_LIGHT, GpuReject, Light, Mesh, ScreenTri, TnlContext, TnlScratch,
    gpu_check, gpu_mesh_check, gpu_screen_matrix,
};
use trino_core::{Color, Mat34, ModelParams, Shading, Vec2, Vec3};

const SCREEN: Vec2 = Vec2::new(400.0, 240.0);

fn camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(1.5, 2.5, -6.0),
        target: Vec3::new(0.0, 0.3, 0.0),
        ..Camera3::default()
    }
}

/// What the vertex shader computes for vertex `i`: screen pixel, depth
/// and color (0..255, before the rasterizer's rounding).
fn shader_vertex(
    ctx: &TnlContext,
    mesh: &Mesh,
    model: &Mat34,
    params: &ModelParams,
    i: usize,
) -> (Vec2, f32, [f32; 4]) {
    let d = gpu_check(ctx, mesh, model, params).expect("GPU path");
    let mv = ctx.projection().0.mul(model);
    let m = gpu_screen_matrix(ctx, &mv);
    let p = mesh.position(i);
    let clip: [f32; 4] =
        core::array::from_fn(|r| m[r][0] * p.x + m[r][1] * p.y + m[r][2] * p.z + m[r][3]);
    let screen = Vec2::new(clip[0] / clip[3], clip[1] / clip[3]);
    let depth = clip[2] / clip[3];
    let n = mesh.normal(i);
    let lit = (n.x * d.light.x + n.y * d.light.y + n.z * d.light.z).max(0.0);
    let c = mesh.color(i);
    let base = [c.r, c.g, c.b, c.a];
    let f = (d.fog[0] * p.x + d.fog[1] * p.y + d.fog[2] * p.z + d.fog[3]).clamp(0.0, 1.0);
    let color = core::array::from_fn(|k| {
        let v = base[k] as f32 / 255.0 * d.tint[k];
        let v = if k < 3 {
            v * (d.ambient[k] + d.diffuse[k] * lit) + d.emissive[k]
        } else {
            v
        };
        let v = v.min(1.0);
        let v = if k < 3 {
            v + (d.fog_color[k] - v) * f
        } else {
            v
        };
        v * 255.0
    });
    (screen, depth, color)
}

fn cpu_tris(ctx: &TnlContext, mesh: &Mesh, model: &Mat34, params: &ModelParams) -> Vec<ScreenTri> {
    let mut out = Vec::new();
    let mut scratch = TnlScratch::new();
    render3d::tessellate_with(ctx, &mut scratch, mesh, model, params, &mut |t| out.push(t));
    out
}

/// Every CPU vertex has a shader vertex at the same pixel, depth and color.
/// Fogged draws split their triangles by depth on the CPU: the new
/// vertices lie inside the mesh's triangles, where the GPU interpolates
/// instead, so only the mesh's own vertices are compared (and every one
/// the CPU kept must be found).
fn assert_same(light: &Light, model: Mat34, params: ModelParams) {
    let blob = bounded_cube();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    gpu_mesh_check(&mesh).unwrap();
    let ctx = TnlContext::new(&camera(), light, SCREEN);
    let tris = cpu_tris(&ctx, &mesh, &model, &params);
    assert!(!tris.is_empty());
    // The CPU truncates to a byte after lighting and again after the fog.
    let tol = if light.fog.enabled() { 2.0 } else { 1.0 };
    let gpu: Vec<_> = (0..mesh.vertex_count)
        .map(|i| shader_vertex(&ctx, &mesh, &model, &params, i))
        .collect();
    let mut matched = 0;
    for t in &tris {
        for k in 0..3 {
            let (pt, z, c) = (t.pts[k], t.z[k], t.colors[k]);
            let near: Vec<_> = gpu
                .iter()
                .filter(|(p, ..)| (p.x - pt.x).abs() < 1e-2 && (p.y - pt.y).abs() < 1e-2)
                .collect();
            if near.is_empty() && light.fog.enabled() {
                continue;
            }
            assert!(!near.is_empty(), "no shader vertex at {pt:?}");
            let ok = near.iter().any(|(_, gz, gc)| {
                (gz - z).abs() < 1e-5
                    && [c.r, c.g, c.b, c.a]
                        .iter()
                        .zip(gc)
                        .all(|(&cpu, &g)| (cpu as f32 - g).abs() <= tol)
            });
            assert!(ok, "vertex at {pt:?}: cpu z {z} {c:?}, shader {near:?}");
            matched += 1;
        }
    }
    // A front-facing cube corner joins 3 faces of 2 triangles each.
    assert!(matched >= 12, "only {matched} vertices compared");
}

#[test]
fn the_shader_constants_reproduce_the_cpu_path() {
    let model = |rot: Vec3, s: f32, at: Vec3| {
        Mat34::from_rotation_scale_translation(rot, Vec3::new(s, s, s), at)
    };
    let colored = Light {
        color: Color::rgb(255, 200, 150),
        ambient_color: Color::rgb(90, 110, 200),
        ambient: 0.3,
        ..DEFAULT_LIGHT
    };
    let tinted = ModelParams {
        tint: Color::rgb(200, 180, 255),
        emissive: Color::rgb(20, 0, 40),
        ..ModelParams::DEFAULT
    };
    let unlit = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgb(255, 128, 64),
        ..ModelParams::DEFAULT
    };
    // The cube spans view depths ~5.5-8: fog from 5 to 9 reaches every
    // strength but full, and from 2 to 6 it saturates the far half (the
    // clamp).
    let fogged = |near: f32, far: f32, base: &Light| Light {
        fog: render3d::Fog {
            color: Color::rgb(132, 178, 226),
            near,
            far,
        },
        ..*base
    };
    let (fog_mid, fog_full) = (fogged(5.0, 9.0, &DEFAULT_LIGHT), fogged(2.0, 6.0, &colored));
    for (rot, s, at) in [
        (Vec3::ZERO, 1.0, Vec3::ZERO),
        (Vec3::new(0.3, 0.9, -0.2), 1.5, Vec3::new(0.4, -0.2, 1.0)),
        (Vec3::new(0.0, 2.1, 0.0), 0.5, Vec3::new(-1.0, 0.5, -2.0)),
    ] {
        for light in [&DEFAULT_LIGHT, &colored, &fog_mid, &fog_full] {
            for params in [ModelParams::DEFAULT, tinted, unlit] {
                assert_same(light, model(rot, s, at), params);
            }
        }
    }
}

#[test]
fn the_gpu_path_refuses_what_needs_the_cpu() {
    let blob = bounded_cube();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let ctx = TnlContext::new(&camera(), &DEFAULT_LIGHT, SCREEN);
    let at = |p: Vec3| Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, p);
    let check = |m: Mat34, p: ModelParams| gpu_check(&ctx, &mesh, &m, &p).err();
    let d = ModelParams::DEFAULT;
    assert_eq!(check(at(Vec3::ZERO), d), None);
    // Behind the camera: culled like the CPU path's sphere test.
    assert_eq!(
        check(at(Vec3::new(0.0, 0.0, -40.0)), d),
        Some(GpuReject::Offscreen)
    );
    // Around the eye: the CPU path clips at NEAR.
    assert_eq!(check(at(camera().eye), d), Some(GpuReject::NearPlane));
    let flat = ModelParams {
        shading: Shading::Flat,
        ..d
    };
    assert_eq!(check(at(Vec3::ZERO), flat), Some(GpuReject::Flat));
    let glass = ModelParams::tinted(Color::rgba(255, 255, 255, 128));
    assert_eq!(check(at(Vec3::ZERO), glass), Some(GpuReject::Translucent));
    // Double-sided is fine on the GPU (no back-face cull).
    let both = ModelParams {
        double_sided: true,
        ..d
    };
    assert!(
        !gpu_check(&ctx, &mesh, &at(Vec3::ZERO), &both)
            .unwrap()
            .cull_back
    );
    // A stretched lit mesh with sloped normals needs world normals.
    let stretched = Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, 0.7, 0.0),
        Vec3::new(3.0, 1.0, 1.0),
        Vec3::ZERO,
    );
    assert_eq!(
        check(stretched, d),
        None,
        "axis normals light in model space"
    );
    let fog = Light {
        fog: render3d::Fog {
            color: Color::BLACK,
            near: 1.0,
            far: 9.0,
        },
        ..DEFAULT_LIGHT
    };
    // Fog is the shader's (by view depth, per vertex).
    let fogged = TnlContext::new(&camera(), &fog, SCREEN);
    let f = gpu_check(&fogged, &mesh, &at(Vec3::ZERO), &d).unwrap();
    assert!(f.fog != [0.0; 4] && f.fog_color == [0.0; 3]);
    assert_eq!(
        gpu_check(&ctx, &mesh, &at(Vec3::ZERO), &d).unwrap().fog,
        [0.0; 4]
    );
    // A texture on a mesh without UVs draws untextured on the CPU path
    // too (`Mesh::texture_for`): the GPU takes it.
    let tex_param = ModelParams {
        texture: Some(trino_core::SpriteId(7)),
        ..d
    };
    assert_eq!(check(at(Vec3::ZERO), tex_param), None);
    // Meshes: textured and skinned ones stay on the CPU.
    let tex = textured_cube(trino_core::SpriteId(7));
    let tex = Mesh::from_tmdl(&tex).unwrap();
    assert_eq!(gpu_mesh_check(&tex), Err(GpuReject::TexturedMesh));
    let rig = rigged_cube();
    let rig = Mesh::from_tmdl(&rig).unwrap();
    assert_eq!(gpu_mesh_check(&rig), Err(GpuReject::Skinned));
}
