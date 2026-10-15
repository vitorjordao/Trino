//! The core's deterministic math, checked against std's: the software
//! sin/cos/sqrt/atan2 accuracy bounds the docs promise, and the vector,
//! rect, color, matrix and quaternion helpers games and the T&L use.

use trino_core::math3d::{self, Mat34, Quat};
use trino_core::{Color, Rect, Vec2, Vec3};

#[test]
fn software_trig_and_sqrt_stay_within_their_documented_error() {
    let mut x = -20.0f32;
    while x < 20.0 {
        assert!((math3d::sin(x) - x.sin()).abs() < 1.2e-3, "sin({x})");
        assert!((math3d::cos(x) - x.cos()).abs() < 1.2e-3, "cos({x})");
        x += 0.0137;
    }
    assert_eq!((math3d::sin(0.0), math3d::cos(0.0)), (0.0, 1.0));
    for v in [1e-6f32, 0.25, 1.0, 2.0, 9.0, 1234.5, 1e6] {
        assert!(
            (math3d::sqrt(v) - v.sqrt()).abs() / v.sqrt() < 1e-5,
            "sqrt({v})"
        );
    }
    assert_eq!(math3d::sqrt(0.0), 0.0);
    assert_eq!(math3d::sqrt(-4.0), 0.0, "negative input is clamped");
    for i in 0..64 {
        let a = i as f32 / 64.0 * core::f32::consts::TAU - core::f32::consts::PI + 0.01;
        let (y, x) = (a.sin() * 3.0, a.cos() * 3.0);
        assert!(
            (math3d::atan2(y, x) - y.atan2(x)).abs() < 5e-3,
            "atan2 at {a}"
        );
    }
    assert_eq!(math3d::atan2(0.0, 0.0), 0.0);
    assert!((math3d::atan2(1.0, 0.0) - core::f32::consts::FRAC_PI_2).abs() < 5e-3);
    assert!((math3d::atan2(0.0, -1.0) - core::f32::consts::PI).abs() < 5e-3);
}

#[test]
fn vectors_rects_and_colors() {
    let a = Vec2::new(1.0, 2.0);
    let mut b = a + Vec2::new(3.0, 4.0);
    assert_eq!(b, Vec2::new(4.0, 6.0));
    b -= Vec2::ONE;
    assert_eq!(b, Vec2::new(3.0, 5.0));
    b += Vec2::ONE;
    assert_eq!((b - a) * 0.5, Vec2::new(1.5, 2.0));
    assert_eq!(-a, Vec2::new(-1.0, -2.0));
    assert_eq!((a.dot(Vec2::new(2.0, 1.0)), a.length_squared()), (4.0, 5.0));
    assert_eq!(Vec2::ZERO + Vec2::ONE, Vec2::ONE);

    let mut v = Vec3::new(1.0, 2.0, 3.0);
    v += Vec3::ONE;
    v -= Vec3::new(0.0, 1.0, 0.0);
    assert_eq!(v, Vec3::new(2.0, 2.0, 4.0));
    assert_eq!(-v * 0.5, Vec3::new(-1.0, -1.0, -2.0));
    assert_eq!(v - Vec3::ONE + Vec3::ZERO, Vec3::new(1.0, 1.0, 3.0));
    assert_eq!(
        Vec3::new(1.0, 0.0, 0.0).cross(Vec3::new(0.0, 1.0, 0.0)),
        Vec3::new(0.0, 0.0, 1.0)
    );
    assert_eq!(Vec3::new(1.0, 2.0, 3.0).dot(Vec3::new(4.0, 5.0, 6.0)), 32.0);
    let n = Vec3::new(3.0, 0.0, 4.0).normalized();
    assert!((n.x - 0.6).abs() < 1e-5 && (n.z - 0.8).abs() < 1e-5);
    assert_eq!(
        Vec3::ZERO.normalized(),
        Vec3::ZERO,
        "no NaN for zero vectors"
    );

    let r = Rect::new(10.0, 10.0, 20.0, 5.0);
    assert!(r.contains(Vec2::new(10.0, 10.0)) && !r.contains(Vec2::new(30.0, 12.0)));
    assert!(r.intersects(&Rect::new(29.0, 14.0, 5.0, 5.0)));
    assert!(
        !r.intersects(&Rect::new(30.0, 10.0, 5.0, 5.0)),
        "edges touching"
    );
    assert_eq!(Rect::default(), Rect::new(0.0, 0.0, 0.0, 0.0));

    assert_eq!(Color::default(), Color::WHITE);
    assert_eq!(Color::rgb(1, 2, 3), Color::rgba(1, 2, 3, 255));
    assert_eq!(Color::TRANSPARENT.a, 0);
    assert_eq!(Color::WHITE.to_rgba5551(), 0xFFFF);
    assert_eq!(
        Color::BLACK.to_rgba5551(),
        0x0001,
        "opaque black keeps the alpha bit"
    );
    assert_eq!(Color::rgba(255, 0, 0, 0).to_rgba5551(), 0xF800);
    assert_eq!(Color::rgba(0, 0, 255, 128).to_rgba5551(), 0x003F);
}

#[test]
fn matrices_compose_transform_and_look() {
    let t = Mat34::from_rotation_scale_translation(
        Vec3::ZERO,
        Vec3::new(2.0, 3.0, 4.0),
        Vec3::new(1.0, 1.0, 1.0),
    );
    assert_eq!(t.transform_point(Vec3::ONE), Vec3::new(3.0, 4.0, 5.0));
    assert_eq!(
        t.transform_dir(Vec3::ONE),
        Vec3::new(2.0, 3.0, 4.0),
        "no translation"
    );
    let s = t.axis_scales();
    assert!((s.x - 2.0).abs() < 1e-4 && (s.z - 4.0).abs() < 1e-4);
    assert_eq!(t.axis_scales_sq(), Vec3::new(4.0, 9.0, 16.0));
    assert!((t.max_scale() - 4.0).abs() < 1e-4);
    // mul applies the right-hand matrix first.
    let move_x =
        Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(10.0, 0.0, 0.0));
    assert_eq!(
        move_x.mul(&t).transform_point(Vec3::ZERO),
        Vec3::new(11.0, 1.0, 1.0)
    );
    assert_eq!(
        t.mul(&move_x).transform_point(Vec3::ZERO),
        Vec3::new(21.0, 1.0, 1.0)
    );
    assert_eq!(Mat34::IDENTITY.mul(&t), t);

    // A quarter turn about Y maps +Z to +X.
    let yaw = Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, core::f32::consts::FRAC_PI_2, 0.0),
        Vec3::ONE,
        Vec3::ZERO,
    );
    let p = yaw.transform_point(Vec3::new(0.0, 0.0, 1.0));
    assert!((p.x - 1.0).abs() < 2e-3 && p.z.abs() < 2e-3, "{p:?}");

    // look_at: the target lands on +Z in view space, +X stays +X.
    let view = Mat34::look_at(
        Vec3::new(0.0, 0.0, -5.0),
        Vec3::ZERO,
        Vec3::new(0.0, 1.0, 0.0),
    );
    let o = view.transform_point(Vec3::ZERO);
    assert!(o.x.abs() < 1e-5 && o.y.abs() < 1e-5 && (o.z - 5.0).abs() < 1e-4);
    let right = view.transform_point(Vec3::new(1.0, 0.0, 0.0));
    assert!(right.x > 0.99);
}

#[test]
fn quaternions_interpolate_and_build_trs_matrices() {
    let id = Quat::IDENTITY;
    assert_eq!(Mat34::from_trs(Vec3::ZERO, id, Vec3::ONE), Mat34::IDENTITY);
    // 90 degrees about Z.
    let h = core::f32::consts::FRAC_1_SQRT_2;
    let qz = Quat::new(0.0, 0.0, h, h);
    let m = Mat34::from_trs(Vec3::new(0.0, 0.0, 5.0), qz, Vec3::new(2.0, 2.0, 2.0));
    let p = m.transform_point(Vec3::new(1.0, 0.0, 0.0));
    assert!(
        p.x.abs() < 1e-5 && (p.y - 2.0).abs() < 1e-5 && (p.z - 5.0).abs() < 1e-6,
        "{p:?}"
    );
    // nlerp: halfway is 45 degrees; the shortest arc is taken.
    let half = id.nlerp(qz, 0.5);
    let len = half.dot(half);
    assert!((len - 1.0).abs() < 1e-4);
    let r = Mat34::from_trs(Vec3::ZERO, half, Vec3::ONE).transform_point(Vec3::new(1.0, 0.0, 0.0));
    assert!((r.x - h).abs() < 1e-3 && (r.y - h).abs() < 1e-3, "{r:?}");
    let neg = Quat::new(0.0, 0.0, -h, -h); // same rotation, opposite sign
    let short = id.nlerp(neg, 0.5);
    assert!(
        (short.z.abs() - half.z.abs()).abs() < 1e-4 && (short.w.abs() - half.w.abs()).abs() < 1e-4
    );
    let start = id.nlerp(qz, 0.0);
    assert!(
        start.z.abs() < 1e-6 && (start.w - 1.0).abs() < 1e-4,
        "{start:?}"
    );
    let n = Quat::new(0.0, 0.0, 0.0, 2.0).normalized();
    assert!((n.w - 1.0).abs() < 1e-5);
    assert_eq!(Quat::new(0.0, 0.0, 0.0, 0.0).normalized(), Quat::IDENTITY);
}

/// `math3d::to_i32` is `as i32` (saturating, NaN → 0) for every float:
/// the special values, the edges of the range, and a sweep through the
/// bit patterns.
#[test]
fn to_i32_matches_the_saturating_cast_everywhere() {
    let edge = 2_147_483_648.0f32;
    for v in [
        0.0f32,
        -0.0,
        0.4,
        -0.6,
        1.0,
        -1.0,
        65535.99,
        -32768.5,
        edge,
        -edge,
        edge - 128.0,
        -edge + 128.0,
        f32::MAX,
        f32::MIN,
        f32::MIN_POSITIVE,
        1e-40,
        f32::INFINITY,
        f32::NEG_INFINITY,
        f32::NAN,
        -f32::NAN,
    ] {
        assert_eq!(math3d::to_i32(v), v as i32, "{v:e}");
    }
    let mut bits = 0u32;
    loop {
        let v = f32::from_bits(bits);
        assert_eq!(math3d::to_i32(v), v as i32, "bits {bits:#010x}");
        match bits.checked_add(0x0000_3FFB) {
            Some(b) => bits = b,
            None => break,
        }
    }
}
