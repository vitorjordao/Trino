//! Physics 2.1 (issue #115): triangle meshes in fixed point. A
//! `CharacterController<Fx>` walks an [`FxMesh`] (a mesh collider converted
//! to Q16.16 at load) with integer arithmetic only: it follows the f32
//! controller on the f32 mesh within a frame of slack, its support and
//! blocking queries agree with the f32 mesh's, and the course's hash is
//! pinned (the console self-test pins the same course on every target).

use trino_core::physics::{
    Box3, CharacterController, ControllerConfig, Fx, FxMesh, FxNode, FxTri, MeshCollider,
    MeshInstance, MeshQuery, Scalar, Surface, V3, tcol_bytes, write_tcol,
};
use trino_core::{Mat34, Vec3};

fn tcol(verts: &[Vec3], tris: &[[u16; 3]]) -> Vec<u8> {
    let mut out = vec![0u8; tcol_bytes(verts.len(), tris.len())];
    let mut order = vec![0u32; tris.len()];
    let n = write_tcol(verts, tris, &mut order, &mut out).unwrap();
    out.truncate(n);
    out
}

/// A 30° ramp (6 m) onto a deck, a wall at the deck's end, and a bumpy
/// field beside it.
fn level() -> Vec<u8> {
    let rise = 6.0 * 30f32.to_radians().tan();
    let mut verts = vec![
        Vec3::new(0.0, 0.0, -3.0),
        Vec3::new(6.0, rise, -3.0),
        Vec3::new(6.0, rise, 3.0),
        Vec3::new(0.0, 0.0, 3.0),
        Vec3::new(12.0, rise, -3.0),
        Vec3::new(12.0, rise, 3.0),
        Vec3::new(12.0, 0.0, -3.0),
        Vec3::new(12.0, rise + 4.0, -3.0),
        Vec3::new(12.0, rise + 4.0, 3.0),
        Vec3::new(12.0, 0.0, 3.0),
    ];
    let mut tris: Vec<[u16; 3]> = vec![
        [0, 2, 1],
        [0, 3, 2],
        [1, 2, 5],
        [1, 5, 4],
        [6, 7, 8],
        [6, 8, 9],
    ];
    // The field: z in 4..12, x in -4..12, gentle bumps.
    let base = verts.len() as u16;
    let (nx, nz) = (9u16, 5u16);
    for j in 0..nz {
        for i in 0..nx {
            let (x, z) = (-4.0 + 2.0 * i as f32, 4.0 + 2.0 * j as f32);
            verts.push(Vec3::new(x, 0.25 * (x * 0.8).sin() * (z * 0.6).cos(), z));
        }
    }
    for j in 0..nz - 1 {
        for i in 0..nx - 1 {
            let a = base + j * nx + i;
            tris.push([a, a + nx, a + 1]);
            tris.push([a + 1, a + nx, a + nx + 1]);
        }
    }
    tcol(&verts, &tris)
}

fn placed(mesh: MeshCollider<'_>) -> MeshInstance<'_> {
    MeshInstance::new(
        mesh,
        Mat34::from_rotation_scale_translation(
            Vec3::new(0.0, 0.4, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(2.0, 0.0, -1.0),
        ),
    )
}

fn convert<'a>(inst: &MeshInstance, t: &'a mut Vec<FxTri>, n: &'a mut Vec<FxNode>) -> FxMesh<'a> {
    let (nt, nn) = FxMesh::size(inst);
    t.resize(nt, FxTri::default());
    n.resize(nn, FxNode::default());
    FxMesh::build(inst, t, n).unwrap()
}

/// Walk up the ramp onto the deck, into the wall, then across the field.
fn drive<S: Scalar>(meshes: &[&dyn MeshQuery<S>]) -> Vec<V3<S>> {
    let floor = [Surface::solid(Box3::new(
        V3::new(S::from_int(-40), S::from_int(-1), S::from_int(-40)),
        V3::new(S::from_int(40), S::ZERO, S::from_int(40)),
    ))];
    let cfg = ControllerConfig {
        step_height: S::from_f32(0.3),
        snap_down: S::from_f32(0.3),
        ..ControllerConfig::new(
            V3::new(S::from_f32(0.3), S::from_f32(0.55), S::from_f32(0.3)),
            S::from_int(25),
            S::from_int(20),
        )
        .with_max_slope_deg(45.0)
    };
    let mut c = CharacterController::new(
        cfg,
        V3::new(S::from_f32(1.0), S::from_f32(0.6), S::from_f32(-1.0)),
    );
    let dt = S::from_f32(1.0 / 60.0);
    let mut path = vec![];
    for f in 0..520 {
        let (vx, vz) = match f {
            0..240 => (3.2, 1.3),
            240..300 => (-2.0, 0.0),
            300..420 => (0.0, 3.0),
            _ => (2.5, 0.4),
        };
        c.vel.x = S::from_f32(vx);
        c.vel.z = S::from_f32(vz);
        c.jump(f == 430, S::from_int(7));
        c.step(dt, &floor, meshes);
        path.push(c.pos);
    }
    path
}

#[test]
fn fixed_point_meshes_follow_the_f32_mesh() {
    let blob = level();
    let inst = placed(MeshCollider::parse(&blob).unwrap());
    let (mut t, mut n) = (vec![], vec![]);
    let fx = convert(&inst, &mut t, &mut n);
    let a = drive::<f32>(&[&inst]);
    let b = drive::<Fx>(&[&fx]);
    let d = |p: &V3<f32>, q: &V3<Fx>| {
        let q = q.to_f32();
        (p.x - q.x)
            .abs()
            .max((p.y - q.y).abs())
            .max((p.z - q.z).abs())
    };
    let mut worst: f32 = 0.0;
    for (i, p) in a.iter().enumerate() {
        let near = (i.saturating_sub(1)..(i + 2).min(b.len()))
            .map(|j| d(p, &b[j]))
            .fold(f32::MAX, f32::min);
        worst = worst.max(near);
    }
    assert!(worst < 0.1, "Fx drifted {worst} m from f32");
    // Both climbed most of the way up the ramp, to the same height.
    let top = b.iter().map(|p| p.y.to_f32()).fold(0.0, f32::max);
    let top_f32 = a.iter().map(|p| p.y).fold(0.0, f32::max);
    assert!(
        top > 2.0 && (top - top_f32).abs() < 0.05,
        "highest {top} vs {top_f32}"
    );
}

#[test]
fn fixed_point_mesh_queries_agree_with_f32() {
    let blob = level();
    let inst = placed(MeshCollider::parse(&blob).unwrap());
    let (mut t, mut n) = (vec![], vec![]);
    let fx = convert(&inst, &mut t, &mut n);
    for i in 0..fx.triangle_count() {
        let u = fx.triangle(i).n;
        let l = ((u.x as f64).powi(2) + (u.y as f64).powi(2) + (u.z as f64).powi(2)).sqrt();
        assert!((l / 65536.0 - 1.0).abs() < 1e-3, "unit normal {l}");
    }
    let mut seed = 0x1357_9bdfu32;
    let mut r = |lo: f32, hi: f32| {
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        lo + (seed >> 8) as f32 / (1u32 << 24) as f32 * (hi - lo)
    };
    let (mut same_block, mut heights) = (0, 0);
    for _ in 0..3000 {
        let c = Vec3::new(r(-6.0, 16.0), r(-1.0, 6.0), r(-6.0, 14.0));
        let h = Vec3::new(0.3, 0.55, 0.3);
        let bf = Box3::new(V3::from(c - h), V3::from(c + h));
        let bx = Box3::new(V3::from_f32(c - h), V3::from_f32(c + h));
        let up = c.y + 0.4;
        let sf = inst.support(&bf, up, 0.7);
        let sx = fx.support(&bx, Fx::from_f32(up), 0.7);
        match (sf, sx) {
            (Some(a), Some(b)) => {
                assert!((a.height - b.height.to_f32()).abs() < 2e-3);
                heights += 1;
            }
            (None, None) => {}
            // Only right at a triangle's edge may one see it and not the
            // other.
            (a, b) => {
                let h = a
                    .map(|s| s.height)
                    .or(b.map(|s| s.height.to_f32()))
                    .unwrap();
                assert!(
                    (h - up).abs() < 1e-2 || h.abs() < 1e-2,
                    "{a:?} {b:?} at {c:?}"
                );
            }
        }
        same_block += (inst.blocks(&bf, 0.7) == fx.blocks(&bx, 0.7)) as usize;
    }
    assert!(heights > 300, "{heights}");
    assert!(same_block >= 2990, "blocks agree {same_block}/3000");
}

#[test]
fn fixed_point_mesh_course_is_pinned() {
    let blob = level();
    let inst = placed(MeshCollider::parse(&blob).unwrap());
    let (mut t, mut n) = (vec![], vec![]);
    let fx = convert(&inst, &mut t, &mut n);
    let (mut t2, mut n2) = (vec![], vec![]);
    let again = convert(&inst, &mut t2, &mut n2);
    assert_eq!(fx.triangle_count(), again.triangle_count());
    for i in 0..fx.triangle_count() {
        assert_eq!(fx.triangle(i), again.triangle(i));
    }
    let mut h: u32 = 0x811c_9dc5;
    for p in drive::<Fx>(&[&fx]) {
        for w in [p.x.0, p.y.0, p.z.0] {
            for b in w.to_le_bytes() {
                h = (h ^ b as u32).wrapping_mul(0x0100_0193);
            }
        }
    }
    assert_eq!(h, 0xb0f7_2ed8, "Fx mesh course hash: {h:#010x}");
}
