//! A two-color ambient and a rim light (`Light::ground`, `Light::rim`,
//! Bongo-Kart #181, ADR 0043 §3) through the CPU T&L: vertices facing up
//! take the sky's ambient, down the ground's, and the rim lights the ones
//! that turn away from the camera. The default light and a ground of the
//! sky's color shade as before; the RSP and the PICA200 leave such draws
//! to the CPU T&L, the PC's GPU T&L keeps them.

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, GpuReject, Light, Mesh, RspReject, ScreenTri, TnlContext, TnlScratch,
    gpu_check, pc_gpu_check, rsp_check, tessellate_with,
};
use trino_core::{Color, DynVertex, DynamicMesh, Mat34, ModelParams, Rim, Shading, Vec2, Vec3};

/// One triangle facing `n`, small and in front of the camera, with every
/// vertex's normal `n` (so the shading is the normal's alone).
fn facing(n: Vec3) -> Vec<u8> {
    let c = Color::rgb(200, 200, 200);
    let v = [
        DynVertex::new(Vec3::new(-0.1, -0.1, 0.0), n, c),
        DynVertex::new(Vec3::new(0.1, -0.1, 0.0), n, c),
        DynVertex::new(Vec3::new(0.0, 0.1, 0.0), n, c),
    ];
    let mesh = DynamicMesh::new(&v, &[0u16, 2, 1]);
    let mut b = vec![0; mesh.tmdl_len().unwrap()];
    mesh.write_tmdl(&mut b).unwrap();
    b
}

fn camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 0.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    }
}

/// The color of the triangle's first vertex under `light`.
fn shade(light: &Light, n: Vec3) -> Color {
    let blob = facing(n);
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let ctx = TnlContext::new(&camera(), light, Vec2::new(320.0, 240.0));
    let params = ModelParams {
        shading: Shading::Gouraud,
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    let mut out: Vec<ScreenTri> = Vec::new();
    let mut scratch = TnlScratch::new();
    tessellate_with(
        &ctx,
        &mut scratch,
        &mesh,
        &Mat34::IDENTITY,
        &params,
        &mut |t| out.push(t),
    );
    out.first().expect("the triangle is drawn").colors[0]
}

const UP: Vec3 = Vec3::new(0.0, 1.0, 0.0);
const DOWN: Vec3 = Vec3::new(0.0, -1.0, 0.0);
const TOWARD: Vec3 = Vec3::new(0.0, 0.0, -1.0);
const SIDE: Vec3 = Vec3::new(1.0, 0.0, 0.0);

/// Full ambient and no sun: the ambient alone colors the vertex.
fn ambient_only() -> Light {
    Light {
        ambient: 1.0,
        ambient_color: Color::rgb(255, 255, 255),
        ..DEFAULT_LIGHT
    }
}

#[test]
fn the_default_light_has_no_terms_and_shades_as_before() {
    assert!(!DEFAULT_LIGHT.has_terms());
    assert_eq!(Light::DEFAULT, DEFAULT_LIGHT);
    assert!(DEFAULT_LIGHT.ground.is_none() && DEFAULT_LIGHT.rim == Rim::NONE);
    // A ground of the sky's own color is the one-color ambient (the lit
    // copy's world normals round within one step of the static path's).
    let light = Light {
        ambient_color: Color::rgb(120, 140, 210),
        ..DEFAULT_LIGHT
    };
    let same = Light {
        ground: Some(light.ambient_color),
        ..light
    };
    assert!(same.has_terms());
    for n in [UP, DOWN, TOWARD, SIDE, Vec3::new(0.6, 0.0, -0.8)] {
        let (a, b) = (shade(&light, n), shade(&same, n));
        let d =
            a.r.abs_diff(b.r)
                .max(a.g.abs_diff(b.g))
                .max(a.b.abs_diff(b.b));
        assert!(d <= 1, "{n:?}: {a:?} vs {b:?}");
    }
}

#[test]
fn the_sky_lights_what_faces_up_and_the_ground_what_faces_down() {
    let light = Light {
        ambient_color: Color::rgb(255, 255, 255),
        ground: Some(Color::rgb(0, 0, 255)),
        ..ambient_only()
    };
    // 200 × the factor (the triangle's color is 200 gray).
    assert_eq!(shade(&light, UP), Color::rgb(200, 200, 200));
    assert_eq!(shade(&light, DOWN), Color::rgb(0, 0, 200));
    let side = shade(&light, TOWARD);
    assert!(
        side.r.abs_diff(100) <= 1 && side.g.abs_diff(100) <= 1,
        "{side:?}"
    );
    assert_eq!(side.b, 200);
    // Without a ground, the old uniform ambient.
    let plain = Light {
        ground: None,
        ..light
    };
    for n in [UP, DOWN, TOWARD] {
        assert_eq!(shade(&plain, n), Color::rgb(200, 200, 200));
    }
}

#[test]
fn the_rim_lights_what_turns_away_from_the_camera() {
    // No ambient, the sun traveling toward the camera: unlit faces.
    let dark = Light {
        ambient: 0.0,
        dir: Vec3::new(0.0, 0.0, -1.0),
        ..DEFAULT_LIGHT
    };
    let rim = Light {
        rim: Rim {
            color: Color::rgb(255, 128, 0),
            strength: 1.0,
        },
        ..dark
    };
    assert_eq!(shade(&dark, TOWARD), Color::BLACK);
    // Facing the camera (n · v ≈ 1, the vertex 0.1 m off its axis): almost
    // nothing.
    let front = shade(&rim, TOWARD);
    assert!(front.r <= 2, "{front:?}");
    // Edge on (n · v ≈ 0): the rim's color at full strength.
    let edge = shade(&rim, SIDE);
    assert!(
        edge.r >= 190 && edge.g.abs_diff(edge.r / 2) <= 2 && edge.b == 0,
        "{edge:?}"
    );
    // Between: (1 - n·v)², n·v = 0.8 → 0.04.
    let half = shade(&rim, Vec3::new(0.6, 0.0, -0.8));
    assert!(half.r > front.r && half.r < 20, "{half:?}");
    // Strength scales it.
    let soft = Light {
        rim: Rim {
            strength: 0.5,
            ..rim.rim
        },
        ..rim
    };
    let e2 = shade(&soft, SIDE);
    assert!(e2.r.abs_diff(edge.r / 2) <= 2, "{e2:?} vs {edge:?}");
}

#[test]
fn the_console_paths_leave_the_terms_to_the_cpu_and_the_pc_gpu_keeps_them() {
    let blob = facing(TOWARD);
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let m = Mat34::IDENTITY;
    let lit = ModelParams::DEFAULT;
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let plain = TnlContext::new(&camera(), &DEFAULT_LIGHT, Vec2::new(320.0, 240.0));
    assert!(rsp_check(&plain, &mesh, &m, &lit).is_ok());
    assert!(gpu_check(&plain, &mesh, &m, &lit).is_ok());
    for light in [
        Light {
            ground: Some(Color::BLACK),
            ..DEFAULT_LIGHT
        },
        Light {
            rim: Rim {
                color: Color::WHITE,
                strength: 0.3,
            },
            ..DEFAULT_LIGHT
        },
    ] {
        let ctx = TnlContext::new(&camera(), &light, Vec2::new(320.0, 240.0));
        assert_eq!(
            rsp_check(&ctx, &mesh, &m, &lit).err(),
            Some(RspReject::LightTerms)
        );
        assert_eq!(
            gpu_check(&ctx, &mesh, &m, &lit).err(),
            Some(GpuReject::LightTerms)
        );
        // Unlit draws ignore the light.
        assert!(rsp_check(&ctx, &mesh, &m, &unlit).is_ok());
        assert!(gpu_check(&ctx, &mesh, &m, &unlit).is_ok());
        assert!(pc_gpu_check(&ctx, &mesh, &m, &lit).is_ok());
    }
    assert_eq!(RspReject::LightTerms.label(), "sky/ground or rim light");
    assert_eq!(
        GpuReject::ALL[GpuReject::LightTerms.index()],
        GpuReject::LightTerms
    );
}
