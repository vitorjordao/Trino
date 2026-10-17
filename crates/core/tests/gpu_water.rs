//! The GPU water constants (`render3d::gpu_water`, ADR 0015) against the
//! CPU water T&L they stand in for: evaluating them the way the 3DS's
//! `water.v.pica` does (same operations, in f32) lands every vertex where
//! the CPU path puts it, with its color, under a swell, a ripple, fog,
//! a colored light and a glint. The shader's own float24 precision is left
//! to the Azahar run (the `water` self-test step).

mod common;

use common::*;
use trino_core::render3d::{
    self, Camera3, GpuReject, GpuWater, Light, Mesh, ScreenTri, TnlContext, TnlScratch, gpu_water,
};
use trino_core::water::sin_turns;
use trino_core::{
    Color, Fog, Mat34, ModelParams, Ripple, Shading, SpriteId, Vec2, Vec3, Water, WaterState,
};

const SCREEN: Vec2 = Vec2::new(400.0, 240.0);

/// A 4 × 4 m sheet at y 0 (8 × 8 quads) with UVs, translucent blue.
fn sheet() -> Vec<u8> {
    let n = 8u16;
    let (mut pos, mut uv, mut idx) = (Vec::new(), Vec::new(), Vec::new());
    for j in 0..=n {
        for i in 0..=n {
            let (x, z) = (i as f32 * 0.5 - 2.0, j as f32 * 0.5 - 2.0);
            pos.push(Vec3::new(x, 0.0, z));
            uv.extend_from_slice(&[x * 0.5, z * 0.5]);
        }
    }
    for j in 0..n {
        for i in 0..n {
            let a = j * (n + 1) + i;
            idx.extend_from_slice(&[a, a + n + 2, a + 1, a, a + n + 1, a + n + 2]);
        }
    }
    let count = pos.len();
    let mut blob = tmdl(
        &pos,
        &vec![Vec3::new(0.0, 1.0, 0.0); count],
        &vec![[90, 170, 255, 160]; count],
        &idx,
    );
    let mut body = Vec::new();
    f32s(&mut body, &uv);
    section(&mut blob, b"TXUV", &body);
    section(&mut blob, b"TXID", &SpriteId(9).0.to_le_bytes());
    section(&mut blob, b"BNDS", &bnds(Vec3::ZERO, 2.9));
    blob
}

fn ctx() -> TnlContext {
    let camera = Camera3 {
        eye: Vec3::new(0.6, 3.2, -3.4),
        target: Vec3::new(0.0, 0.0, 0.3),
        ..Camera3::default()
    };
    let light = Light {
        dir: Vec3::new(0.3, -0.8, 0.5),
        ambient: 0.4,
        color: Color::rgb(255, 236, 210),
        ambient_color: Color::rgb(180, 200, 255),
        fog: Fog {
            color: Color::rgb(130, 170, 220),
            near: 3.0,
            far: 9.0,
        },
    };
    let ripple = Ripple {
        pos: Vec2::new(0.3, -0.2),
        age: 0.45,
        strength: 0.12,
    };
    TnlContext::new(&camera, &light, SCREEN).with_water(&WaterState::new(1.3, &[ripple], 4))
}

fn params() -> ModelParams {
    ModelParams {
        gloss: 200,
        water: Some(Water {
            flow_speed: Vec2::new(0.1, 0.4),
            detail_flow: Vec2::new(0.3, -0.2),
            wave_amplitude: 0.08,
            wave_frequency: 0.9,
            ripple_responsiveness: 1.0,
        }),
        ..ModelParams::DEFAULT
    }
}

fn dot4(a: [f32; 4], b: [f32; 4]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2] + a[3] * b[3]
}

/// `water.v.pica` for one vertex: screen pixel and color (0..1).
#[allow(clippy::needless_range_loop)] // the shader's lanes, as written there
fn shader(u: &GpuWater, p: Vec3, c: Color) -> (Vec2, [f32; 4]) {
    let p4 = [p.x, p.y, p.z, 1.0];
    let mut w = [
        dot4(u.model[0], p4),
        dot4(u.model[1], p4),
        dot4(u.model[2], p4),
        1.0,
    ];
    let s4 = |t: [f32; 4]| t.map(sin_turns);
    // The swell.
    let t0 = u.wk[0] * (u.wdir[0] * w[0] + u.wdir[1] * w[2]) + u.wk[2];
    let t1 = u.wk[1] * (u.wdir[2] * w[0] + u.wdir[3] * w[2]) + u.wk[3];
    let sc = s4([t0, t1, t0 + 0.25, t1 + 0.25]);
    let mut h = u.wamp[0] * sc[0] + u.wamp[1] * sc[1];
    let (a, b) = (u.wamp[2] * sc[2], u.wamp[3] * sc[3]);
    let mut g = [u.wdir[0] * a + u.wdir[2] * b, u.wdir[1] * a + u.wdir[3] * b];
    // Ripples, one per lane.
    let dx: [f32; 4] = core::array::from_fn(|i| w[0] - u.rx[i]);
    let dz: [f32; 4] = core::array::from_fn(|i| w[2] - u.rz[i]);
    let d2: [f32; 4] = core::array::from_fn(|i| dx[i] * dx[i] + dz[i] * dz[i] + 1e-6);
    let inv: [f32; 4] = d2.map(|v| 1.0 / v.sqrt());
    let d: [f32; 4] = core::array::from_fn(|i| d2[i] * inv[i]);
    let dr: [f32; 4] = core::array::from_fn(|i| d[i] - u.rrad[i]);
    let q = dr.map(|v| v / 0.7);
    let e = q.map(|v| (1.0 - v * v).max(0.0));
    let tr = dr.map(|v| v / 0.55);
    let sn = s4(tr);
    let cs = s4(tr.map(|v| v + 0.25));
    for i in 0..4 {
        h += u.rstr[i] * e[i] * e[i] * cs[i];
        let dh = u.rstr[i]
            * (-4.0 * e[i] * q[i] / 0.7 * cs[i]
                - e[i] * e[i] * core::f32::consts::TAU / 0.55 * sn[i]);
        g[0] += dh * inv[i] * dx[i];
        g[1] += dh * inv[i] * dz[i];
    }
    w[1] += h;
    let n = Vec3::new(-g[0], 1.0, -g[1]).normalized();
    let l = Vec3::new(u.light[0], u.light[1], u.light[2]);
    let lit = n.dot(l).max(0.0);
    let base = [c.r, c.g, c.b, c.a].map(|v| v as f32 / 255.0);
    let mut out: [f32; 4] = core::array::from_fn(|k| base[k] * u.tint[k]);
    for k in 0..3 {
        out[k] = out[k] * (u.ambient[k] + u.diffuse[k] * lit) + u.emissive[k];
    }
    let wp = Vec3::new(w[0], w[1], w[2]);
    let v = (Vec3::new(u.eye[0], u.eye[1], u.eye[2]) - wp).normalized();
    let m = 1.0 - n.dot(v).max(0.0);
    let f = 0.85 * (0.02 + 0.98 * m * m * m * m * m);
    for k in 0..3 {
        out[k] += (u.sky[k] - out[k]) * f;
    }
    out[3] += (1.0 - out[3]) * f;
    let hv = (v + l).normalized();
    let s = n.dot(hv).max(1e-6).powf(u.glint[3]);
    for k in 0..3 {
        out[k] += u.glint[k] * s;
    }
    let fog = dot4(u.fog, w).clamp(0.0, 1.0);
    for k in 0..3 {
        out[k] += (u.fog_color[k] - out[k]) * fog;
    }
    let out = out.map(|v| v.clamp(0.0, 1.0));
    let clip: [f32; 4] = core::array::from_fn(|r| dot4(u.screen[r], w));
    (Vec2::new(clip[0] / clip[3], clip[1] / clip[3]), out)
}

#[test]
fn the_shader_constants_reproduce_the_cpu_water() {
    let blob = sheet();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let model = Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, 0.3, 0.0),
        Vec3::ONE,
        Vec3::new(0.2, -0.1, 0.4),
    );
    let ctx = ctx();
    let p = params();
    let u = gpu_water(&ctx, &mesh, &model, &p, 2.0).expect("the GPU water path");
    assert!(
        u.rstr[0] > 0.0 && u.rstr[1] == 0.0,
        "one ripple: {:?}",
        u.rstr
    );
    let mut cpu: Vec<ScreenTri> = Vec::new();
    let mut scratch = TnlScratch::new();
    render3d::tessellate_with(&ctx, &mut scratch, &mesh, &model, &p, &mut |t| cpu.push(t));
    let gpu: Vec<(Vec2, [f32; 4])> = (0..mesh.vertex_count)
        .map(|i| shader(&u, mesh.position(i), mesh.color(i)))
        .collect();
    let mut checked = 0;
    for t in cpu.iter().filter(|t| t.depth > 0.0) {
        for k in 0..3 {
            let (pt, c) = (t.pts[k], t.colors[k]);
            let Some((_, g)) = gpu
                .iter()
                .find(|(s, _)| (s.x - pt.x).abs() < 0.05 && (s.y - pt.y).abs() < 0.05)
            else {
                panic!("no shader vertex at {pt:?}");
            };
            let want = [c.r, c.g, c.b, c.a];
            for ch in 0..4 {
                let got = g[ch] * 255.0;
                assert!(
                    (got - want[ch] as f32).abs() <= 2.0,
                    "channel {ch} at {pt:?}: shader {got}, CPU {}",
                    want[ch]
                );
            }
            checked += 1;
        }
    }
    assert!(checked > 100, "{checked} corners");
}

#[test]
fn the_gpu_water_path_refuses_what_needs_the_cpu() {
    let blob = sheet();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let ctx = ctx();
    let p = params();
    assert_eq!(
        gpu_water(&ctx, &mesh, &Mat34::IDENTITY, &ModelParams::DEFAULT, 1.0).unwrap_err(),
        GpuReject::Water
    );
    // Crossing the near plane: the CPU clips it.
    let near =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(0.6, 3.0, -3.4));
    assert_eq!(
        gpu_water(&ctx, &mesh, &near, &p, 1.0).unwrap_err(),
        GpuReject::NearPlane
    );
    // A long surface whose bounding sphere reaches the camera, though every
    // vertex lies ahead of it: no triangle needs the near plane, the GPU
    // takes it.
    let long = Mat34::from_rotation_scale_translation(
        Vec3::ZERO,
        Vec3::new(8.0, 1.0, 1.0),
        Vec3::new(0.0, 0.0, 2.0),
    );
    assert!(gpu_water(&ctx, &mesh, &long, &p, 1.0).is_ok());
    // Unlit: ambient 1, no diffuse.
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..p
    };
    let u = gpu_water(&ctx, &mesh, &Mat34::IDENTITY, &unlit, 1.0).unwrap();
    assert_eq!(u.ambient[..3], [1.0; 3]);
    assert_eq!(u.diffuse, [0.0; 4]);
}
