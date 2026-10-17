//! The software T&L end to end: TMDL blob → `Mesh` → `tessellate` /
//! `tessellate_with` → screen triangles, checked for culling, back faces,
//! shading modes, tint, emissive, colored light, fog, double-sided draws,
//! near-plane clipping, textures, skinning, plus billboards and the
//! N64 work split helpers (`split_policy`, `rsp_eligible`).

mod common;

use common::*;
use trino_core::render3d::{
    self, Camera3, DEFAULT_LIGHT, Fog, Light, Mesh, ScreenTri, TnlContext, TnlScratch, TnlStats,
    tessellate, tessellate_with,
};
use trino_core::{
    AnimRef, BillboardParams, ClipId, Color, Mat34, ModelParams, Rect, Shading, SpriteId, Vec2,
    Vec3,
};

fn draw(
    blob: &[u8],
    model: &Mat34,
    light: &Light,
    params: &ModelParams,
) -> (Vec<ScreenTri>, TnlStats) {
    let mesh = Mesh::from_tmdl(blob).unwrap();
    let mut out = Vec::new();
    let s = tessellate(
        &mesh,
        model,
        &Camera3::default(),
        light,
        params,
        SCREEN,
        &mut |t| out.push(t),
    );
    (out, s)
}

fn at(p: Vec3) -> Mat34 {
    Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, p)
}

fn area(t: &ScreenTri) -> f32 {
    let p = t.pts;
    (p[1].x - p[0].x) * (p[2].y - p[0].y) - (p[2].x - p[0].x) * (p[1].y - p[0].y)
}

#[test]
fn a_cube_face_on_emits_its_front_face_only() {
    let (tris, s) = draw(
        &bounded_cube(),
        &Mat34::IDENTITY,
        &DEFAULT_LIGHT,
        &ModelParams::DEFAULT,
    );
    assert_eq!(s.tris_in, 12);
    assert_eq!(s.emitted, 2);
    assert_eq!(s.backfaced, 10);
    assert_eq!(s.culled, 0);
    assert_eq!(s.verts, 24, "each vertex transformed once");
    assert_eq!(tris.len(), 2);
    for t in &tris {
        assert!(area(t) > 0.0, "front faces have positive screen area");
        // The front face sits at view depth 4.5, centered on the screen.
        for k in 0..3 {
            assert!((t.w[k] - 4.5).abs() < 1e-3);
            assert!((t.z[k] - (1.0 - 0.05 / t.w[k])).abs() < 1e-5);
            assert!(t.pts[k].x > 100.0 && t.pts[k].x < 220.0);
            assert_eq!(t.uv[k], Vec2::ZERO, "untextured");
        }
        assert!((t.depth - 4.5).abs() < 1e-3);
    }
    // Lit with the default light: darker than the raw vertex color.
    let c = tris[0].colors[0];
    assert!(c.r < 230 && c.r > 0 && c.a == 255, "{c:?}");

    // Rotated 45 degrees about Y: two faces visible.
    let turned = Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, core::f32::consts::FRAC_PI_4, 0.0),
        Vec3::ONE,
        Vec3::ZERO,
    );
    let (_, s) = draw(
        &bounded_cube(),
        &turned,
        &DEFAULT_LIGHT,
        &ModelParams::DEFAULT,
    );
    assert_eq!(s.emitted, 4);
}

#[test]
fn meshes_outside_the_frustum_are_culled_whole() {
    for p in [
        Vec3::new(40.0, 0.0, 0.0),
        Vec3::new(-40.0, 0.0, 0.0),
        Vec3::new(0.0, 40.0, 0.0),
        Vec3::new(0.0, -40.0, 0.0),
        Vec3::new(0.0, 0.0, -20.0), // behind the camera
    ] {
        let (tris, s) = draw(
            &bounded_cube(),
            &at(p),
            &DEFAULT_LIGHT,
            &ModelParams::DEFAULT,
        );
        assert_eq!(s.culled, 1, "{p:?}");
        assert_eq!((s.verts, s.tris_in, tris.len()), (0, 0, 0), "{p:?}");
    }
    // Without a bounding sphere the same draw is not culled up front: the
    // vertices are paid for, then every triangle is clipped away.
    let (tris, s) = draw(
        &cube_tmdl(),
        &at(Vec3::new(40.0, 0.0, 0.0)),
        &DEFAULT_LIGHT,
        &ModelParams::DEFAULT,
    );
    assert_eq!(s.culled, 0);
    assert!(s.verts > 0);
    assert!(tris.is_empty());
}

#[test]
fn double_sided_draws_keep_back_faces() {
    let params = ModelParams {
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    let (tris, s) = draw(&bounded_cube(), &Mat34::IDENTITY, &DEFAULT_LIGHT, &params);
    assert_eq!(s.backfaced, 0);
    assert_eq!(s.emitted, 12);
    assert_eq!(tris.len(), 12);
}

#[test]
fn shading_modes_tint_and_emissive() {
    let face = |params: &ModelParams| {
        let (tris, _) = draw(&bounded_cube(), &Mat34::IDENTITY, &DEFAULT_LIGHT, params);
        tris[0].colors
    };
    // Unlit: vertex color x tint, exactly.
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    assert_eq!(face(&unlit), [Color::rgba(230, 80, 80, 255); 3]);
    let tinted = ModelParams {
        tint: Color::rgba(128, 255, 255, 128),
        ..unlit
    };
    let c = face(&tinted)[0];
    assert_eq!((c.r, c.g, c.b), (115, 80, 80));
    assert_eq!(c.a, 128, "tint alpha multiplies vertex alpha");
    let glow = ModelParams {
        emissive: Color::rgb(40, 10, 0),
        ..unlit
    };
    assert_eq!(
        face(&glow)[0],
        Color::rgba(255, 90, 80, 255),
        "added, saturating"
    );

    // Gouraud and Flat on a flat-shaded cube agree (one normal per face),
    // and both are darker than unlit; flat is uniform per triangle.
    let gouraud = face(&ModelParams::DEFAULT);
    let flat = face(&ModelParams {
        shading: Shading::Flat,
        ..ModelParams::DEFAULT
    });
    assert_eq!(flat[0], flat[1]);
    assert_eq!(flat[1], flat[2]);
    for k in 0..3 {
        assert!(flat[k].r.abs_diff(gouraud[k].r) <= 1);
        assert!(gouraud[k].r < 230);
    }
    // Full ambient light is unlit brightness.
    let bright = Light {
        ambient: 1.0,
        ..DEFAULT_LIGHT
    };
    let (tris, _) = draw(
        &bounded_cube(),
        &Mat34::IDENTITY,
        &bright,
        &ModelParams::DEFAULT,
    );
    assert_eq!(tris[0].colors[0], Color::rgba(230, 80, 80, 255));
}

#[test]
fn colored_light_tints_lit_surfaces() {
    // A white face toward a red key light with a dim blue ambient.
    let white = faces_tmdl(&[(Vec3::new(0.0, 0.0, -1.0), [255, 255, 255, 255])], 1.0);
    let light = Light {
        dir: Vec3::new(0.0, 0.0, 1.0), // travels away from the camera: lights the front
        ambient: 0.2,
        color: Color::rgb(255, 0, 0),
        ambient_color: Color::rgb(0, 0, 255),
        fog: Fog::NONE,
    };
    let (tris, _) = draw(&white, &Mat34::IDENTITY, &light, &ModelParams::DEFAULT);
    let c = tris[0].colors[0];
    // red = ambient 0 + (1 - 0.2) * 1 * 255; green = 0; blue = 0.2 * 255.
    assert_eq!(c.g, 0, "{c:?}");
    assert!(c.r.abs_diff(204) <= 1, "{c:?}");
    assert!(c.b.abs_diff(51) <= 1, "{c:?}");
}

#[test]
fn fog_blends_toward_the_fog_color_with_view_depth() {
    let fogged = |near: f32, far: f32| Light {
        fog: Fog {
            color: Color::rgb(0, 0, 255),
            near,
            far,
        },
        ..DEFAULT_LIGHT
    };
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    // Face at depth 4.5: halfway through fog 1..8 -> half blue.
    let (tris, _) = draw(&bounded_cube(), &Mat34::IDENTITY, &fogged(1.0, 8.0), &unlit);
    let c = tris[0].colors[0];
    assert!(c.r.abs_diff(115) <= 1 && c.b.abs_diff(167) <= 1, "{c:?}");
    // Fog past the object: untouched; fog fully before it: all fog color.
    let (clear, _) = draw(
        &bounded_cube(),
        &Mat34::IDENTITY,
        &fogged(10.0, 20.0),
        &unlit,
    );
    assert_eq!(clear[0].colors[0], Color::rgba(230, 80, 80, 255));
    let (thick, _) = draw(&bounded_cube(), &Mat34::IDENTITY, &fogged(0.1, 1.0), &unlit);
    assert_eq!(thick[0].colors[0], Color::rgba(0, 0, 255, 255));
    // far <= near disables fog.
    assert!(!Fog::NONE.enabled());
    assert!(fogged(1.0, 8.0).fog.enabled());
}

#[test]
fn geometry_crossing_the_near_plane_is_clipped_into_the_guard_band() {
    // A 40 m floor under a camera standing on it: half of it is behind.
    let floor = floor_tmdl(-1.0, 20.0);
    let (tris, s) = draw(
        &floor,
        &Mat34::IDENTITY,
        &DEFAULT_LIGHT,
        &ModelParams::DEFAULT,
    );
    assert_eq!(s.tris_in, 2);
    assert!(s.clipped >= 1, "{s:?}");
    assert!(!tris.is_empty());
    for t in &tris {
        for k in 0..3 {
            let p = t.pts[k];
            // The guard band is 1.5x the screen around its center.
            assert!(p.x >= -80.5 && p.x <= 400.5, "{p:?}");
            assert!(p.y >= -60.5 && p.y <= 300.5, "{p:?}");
            assert!(t.w[k] >= 0.05 - 1e-4, "in front of the near plane");
            assert!((0.0..1.0).contains(&t.z[k]));
        }
        assert!(area(t) > 0.0);
    }
    // A fogged floor also gets depth splits (edges > DEPTH_SPLIT deep).
    let fog = Light {
        fog: Fog {
            color: Color::BLACK,
            near: 1.0,
            far: 30.0,
        },
        ..DEFAULT_LIGHT
    };
    let (split, s) = draw(&floor, &Mat34::IDENTITY, &fog, &ModelParams::DEFAULT);
    assert!(s.splits > 0, "{s:?}");
    assert!(split.len() > tris.len());
}

#[test]
fn textured_draws_carry_uvs_and_view_depth() {
    let (tris, s) = draw(
        &textured_cube(SpriteId(5)),
        &Mat34::IDENTITY,
        &DEFAULT_LIGHT,
        &ModelParams::DEFAULT,
    );
    assert_eq!(s.emitted, tris.len() as u32);
    assert!(!tris.is_empty());
    let mut us = Vec::new();
    for t in &tris {
        for k in 0..3 {
            us.push(t.uv[k].x);
            assert!((-1e-3..=1.001).contains(&t.uv[k].x));
            assert!(t.w[k] > 4.0 && t.w[k] < 5.0);
        }
    }
    assert!(us.iter().any(|&u| u > 0.9) && us.iter().any(|&u| u < 0.1));
}

#[test]
fn skinned_meshes_follow_their_pose() {
    let blob = rigged_cube();
    let top_y = |anim: Option<AnimRef>| {
        let p = ModelParams {
            anim,
            shading: Shading::Unlit,
            ..ModelParams::DEFAULT
        };
        let (tris, s) = draw(&blob, &Mat34::IDENTITY, &DEFAULT_LIGHT, &p);
        assert_eq!(s.tris_in, 12);
        tris.iter()
            .flat_map(|t| t.pts)
            .map(|p| p.y)
            .fold(f32::MAX, f32::min)
    };
    let rest = top_y(None);
    let lifted = top_y(Some(AnimRef::new(ClipId::from_name("lift"), 0.5)));
    assert!(
        lifted < rest - 10.0,
        "the top moved up on screen: {rest} -> {lifted}"
    );

    // Smooth skinning (SKW2): top vertices weighted fully back to the
    // root do not follow the lifted child.
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let mut w = Vec::new();
    for i in 0..mesh.vertex_count {
        w.extend_from_slice(&[0, if mesh.position(i).y > 0.0 { 255 } else { 0 }]);
    }
    let mut smooth = blob.clone();
    section(&mut smooth, b"SKW2", &w);
    let p = ModelParams {
        anim: Some(AnimRef::new(ClipId::from_name("lift"), 0.5)),
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let (tris, _) = draw(&smooth, &Mat34::IDENTITY, &DEFAULT_LIGHT, &p);
    let smooth_top = tris
        .iter()
        .flat_map(|t| t.pts)
        .map(|p| p.y)
        .fold(f32::MAX, f32::min);
    assert!((smooth_top - rest).abs() < 0.5, "{rest} vs {smooth_top}");
}

#[test]
fn tessellate_with_reuses_context_and_scratch_bit_identically() {
    let blob = bounded_cube();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let cam = Camera3 {
        eye: Vec3::new(2.0, 3.0, -6.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    };
    let ctx = TnlContext::new(&cam, &DEFAULT_LIGHT, SCREEN);
    assert!(ctx.matches(&cam, &DEFAULT_LIGHT, SCREEN));
    assert!(!ctx.matches(&Camera3::default(), &DEFAULT_LIGHT, SCREEN));
    assert!(!ctx.matches(&cam, &DEFAULT_LIGHT, Vec2::new(400.0, 240.0)));
    let (view, focal, screen) = ctx.projection();
    assert_eq!(view, cam.view());
    assert_eq!(screen, SCREEN);
    assert!(focal > 0.0);

    let mut scratch = TnlScratch::default();
    let mut total = TnlStats::default();
    let mut a = Vec::new();
    for x in [-1.5f32, 0.0, 1.5] {
        let s = tessellate_with(
            &ctx,
            &mut scratch,
            &mesh,
            &at(Vec3::new(x, 0.0, 0.0)),
            &ModelParams::DEFAULT,
            &mut |t| a.push(t.pts),
        );
        total.add(&s);
    }
    let mut b = Vec::new();
    for x in [-1.5f32, 0.0, 1.5] {
        tessellate(
            &mesh,
            &at(Vec3::new(x, 0.0, 0.0)),
            &cam,
            &DEFAULT_LIGHT,
            &ModelParams::DEFAULT,
            SCREEN,
            &mut |t| b.push(t.pts),
        );
    }
    assert_eq!(a, b);
    assert_eq!(total.emitted as usize, a.len());
    assert_eq!(total.tris_in, 36);
    assert_eq!(total.verts, 72);
}

#[test]
fn painter_keys_order_far_first_then_submission() {
    let mut keys: Vec<u64> = [3.0f32, 10.0, 3.0, 0.5]
        .iter()
        .enumerate()
        .map(|(i, d)| render3d::painter_key(*d, i))
        .collect();
    keys.sort_unstable();
    let order: Vec<usize> = keys.into_iter().map(render3d::painter_index).collect();
    assert_eq!(order, [1, 0, 2, 3]);
}

#[test]
fn billboards_face_the_camera_and_respect_fog_and_regions() {
    let cam = Camera3::default();
    let p = BillboardParams {
        size: Vec2::new(2.0, 1.0),
        ..BillboardParams::DEFAULT
    };
    let uv = render3d::region_uv(Some(Rect::new(16.0, 0.0, 16.0, 16.0)), 64.0, 16.0);
    assert_eq!(uv, [0.25, 0.0, 0.5, 1.0]);
    assert_eq!(render3d::region_uv(None, 64.0, 16.0), [0.0, 0.0, 1.0, 1.0]);
    let [a, b] = render3d::billboard(&cam, &DEFAULT_LIGHT, SCREEN, Vec3::ZERO, &p, uv).unwrap();
    assert!(area(&a) > 0.0 && area(&b) > 0.0);
    // 2 x 1 world units at depth 5: twice as wide as tall on screen.
    let w = a.pts[1].x - a.pts[0].x;
    let h = a.pts[2].y - a.pts[1].y;
    assert!((w / h - 2.0).abs() < 1e-3, "{w} x {h}");
    assert_eq!(a.uv[0], Vec2::new(0.25, 0.0));
    assert_eq!(a.uv[2], Vec2::new(0.5, 1.0));
    assert_eq!(a.colors, [Color::WHITE; 3], "billboards are unlit");
    assert!((a.depth - 5.0).abs() < 1e-4);

    // Rolled a quarter turn: still a quad of the same area.
    let rolled = BillboardParams {
        rotation: core::f32::consts::FRAC_PI_2,
        ..p
    };
    let [r, _] =
        render3d::billboard(&cam, &DEFAULT_LIGHT, SCREEN, Vec3::ZERO, &rolled, uv).unwrap();
    assert!((area(&r) - area(&a)).abs() / area(&a) < 1e-2);

    // Fog tints toward the fog color but keeps the tint's alpha.
    let fog = Light {
        fog: Fog {
            color: Color::rgb(0, 0, 0),
            near: 0.0,
            far: 10.0,
        },
        ..DEFAULT_LIGHT
    };
    let half = BillboardParams {
        tint: Color::rgba(200, 200, 200, 100),
        ..p
    };
    let [f, _] = render3d::billboard(&cam, &fog, SCREEN, Vec3::ZERO, &half, uv).unwrap();
    let fc = f.colors[0];
    assert!(
        fc.r.abs_diff(100) <= 1 && fc.r == fc.g && fc.g == fc.b,
        "{fc:?}"
    );
    assert_eq!(fc.a, 100);

    // Behind the camera, off screen, or so close it would be huge: none.
    assert!(
        render3d::billboard(
            &cam,
            &DEFAULT_LIGHT,
            SCREEN,
            Vec3::new(0.0, 0.0, -9.0),
            &p,
            uv
        )
        .is_none()
    );
    assert!(
        render3d::billboard(
            &cam,
            &DEFAULT_LIGHT,
            SCREEN,
            Vec3::new(90.0, 0.0, 0.0),
            &p,
            uv
        )
        .is_none()
    );
    assert!(
        render3d::billboard(
            &cam,
            &DEFAULT_LIGHT,
            SCREEN,
            Vec3::new(0.0, 90.0, 0.0),
            &p,
            uv
        )
        .is_none()
    );
    let huge = BillboardParams {
        size: Vec2::new(500.0, 500.0),
        ..p
    };
    assert!(
        render3d::billboard(
            &cam,
            &DEFAULT_LIGHT,
            SCREEN,
            Vec3::new(0.0, 0.0, -4.8),
            &huge,
            uv
        )
        .is_none()
    );
    // The context variant gives the same triangles.
    let ctx = TnlContext::new(&cam, &DEFAULT_LIGHT, SCREEN);
    let [c, _] = render3d::billboard_with(&ctx, Vec3::ZERO, &p, uv).unwrap();
    assert_eq!(c.pts, a.pts);
}

#[test]
fn split_policy_and_rsp_eligibility_follow_the_draw() {
    let cube = bounded_cube();
    let tex = textured_cube(SpriteId(1));
    let (cube_m, tex_m) = (
        Mesh::from_tmdl(&cube).unwrap(),
        Mesh::from_tmdl(&tex).unwrap(),
    );
    let plain = TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN);
    let fog = Light {
        fog: Fog {
            color: Color::BLACK,
            near: 1.0,
            far: 9.0,
        },
        ..DEFAULT_LIGHT
    };
    let fogged = TnlContext::new(&Camera3::default(), &fog, SCREEN);
    let opaque = ModelParams::DEFAULT;
    let glass = ModelParams::tinted(Color::rgba(255, 255, 255, 128));
    assert!(!render3d::split_policy(&plain, &cube_m, &opaque));
    assert!(render3d::split_policy(&fogged, &cube_m, &opaque));
    assert!(render3d::split_policy(&plain, &cube_m, &glass));
    assert!(render3d::split_policy(&plain, &tex_m, &opaque));

    // RSP: a static opaque untextured mesh inside the view is eligible.
    assert!(render3d::rsp_mesh_supported(&cube_m));
    assert!(!render3d::rsp_mesh_supported(&tex_m));
    let rigged = rigged_cube();
    assert!(!render3d::rsp_mesh_supported(
        &Mesh::from_tmdl(&rigged).unwrap()
    ));
    let d = render3d::rsp_eligible(&plain, &cube_m, &Mat34::IDENTITY, &opaque).unwrap();
    assert!(d.cull_back);
    assert_eq!(d.ambient, DEFAULT_LIGHT.ambient);
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..opaque
    };
    let u = render3d::rsp_eligible(&plain, &cube_m, &Mat34::IDENTITY, &unlit).unwrap();
    assert_eq!((u.light, u.ambient), (Vec3::ZERO, 1.0));
    for (ctx, params, m) in [
        (&fogged, opaque, Mat34::IDENTITY),
        (&plain, glass, Mat34::IDENTITY),
        (
            &plain,
            ModelParams {
                shading: Shading::Flat,
                ..opaque
            },
            Mat34::IDENTITY,
        ),
        (
            &plain,
            ModelParams {
                double_sided: true,
                ..opaque
            },
            Mat34::IDENTITY,
        ),
        (
            &plain,
            ModelParams {
                texture: Some(SpriteId(1)),
                ..opaque
            },
            Mat34::IDENTITY,
        ),
        (&plain, opaque, at(Vec3::new(0.0, 0.0, -4.7))), // crosses the near plane
        (&plain, opaque, at(Vec3::new(300.0, 0.0, 0.0))), // outside the band
    ] {
        assert!(
            render3d::rsp_eligible(ctx, &cube_m, &m, &params).is_none(),
            "{params:?} {m:?}"
        );
    }
    // A mesh without a bounding sphere never goes to the RSP.
    let unbounded = cube_tmdl();
    assert!(
        render3d::rsp_eligible(
            &plain,
            &Mesh::from_tmdl(&unbounded).unwrap(),
            &Mat34::IDENTITY,
            &opaque
        )
        .is_none()
    );
    // The segment split covers every triangle exactly once.
    let mut tris = 0;
    let segs = render3d::rsp_split(&cube_m, &mut |verts, t| {
        assert!(verts.len() <= render3d::RSP_SEG_VERTS);
        tris += t.len();
    });
    assert_eq!((segs, tris), (1, 12));
}

#[test]
fn degenerate_flat_skinned_and_multi_segment_meshes() {
    // A zero-area triangle drawn double-sided is dropped as degenerate.
    let p = Vec3::new(0.0, 0.0, 0.0);
    let n = Vec3::new(0.0, 0.0, -1.0);
    let sliver = tmdl(
        &[p, Vec3::new(1.0, 0.0, 0.0), Vec3::new(2.0, 0.0, 0.0)],
        &[n; 3],
        &[[255; 4]; 3],
        &[0, 1, 2],
    );
    let both = ModelParams {
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    let (tris, s) = draw(&sliver, &Mat34::IDENTITY, &DEFAULT_LIGHT, &both);
    assert_eq!((tris.len(), s.backfaced, s.emitted), (0, 1, 0));

    // Flat shading on a smooth-skinned mesh: one color per triangle from
    // the blended world positions.
    let blob = rigged_cube();
    let mesh = Mesh::from_tmdl(&blob).unwrap();
    let mut w = Vec::new();
    for i in 0..mesh.vertex_count {
        w.extend_from_slice(&[0, if mesh.position(i).y > 0.0 { 128 } else { 0 }]);
    }
    let mut smooth = blob.clone();
    section(&mut smooth, b"SKW2", &w);
    let flat = ModelParams {
        shading: Shading::Flat,
        anim: Some(AnimRef::new(ClipId::from_name("lift"), 0.5)),
        ..ModelParams::DEFAULT
    };
    let (tris, s) = draw(&smooth, &Mat34::IDENTITY, &DEFAULT_LIGHT, &flat);
    assert!(s.emitted > 0);
    for t in &tris {
        assert_eq!(t.colors[0], t.colors[1]);
        assert_eq!(t.colors[1], t.colors[2]);
    }

    // Three cubes side by side (72 vertices) need several RSP segments of
    // at most 32 vertices; every triangle lands in exactly one.
    let mut faces = Vec::new();
    for _ in 0..3 {
        faces.extend_from_slice(&CUBE_FACES);
    }
    let big = faces_tmdl(&faces, 1.0);
    let big_mesh = Mesh::from_tmdl(&big).unwrap();
    let (mut tris_seen, mut max_verts) = (0, 0);
    let segs = render3d::rsp_split(&big_mesh, &mut |verts, t| {
        max_verts = max_verts.max(verts.len());
        tris_seen += t.len();
    });
    assert_eq!(tris_seen, 36);
    assert!(segs >= 3, "{segs}");
    assert!(max_verts <= render3d::RSP_SEG_VERTS);
}

/// `rsp_eligible_mv` (the backend passes its `view · model` product) gives
/// exactly `rsp_eligible`'s answers: the N64 split must stay the one the
/// host tests predict.
#[test]
fn rsp_eligibility_with_a_given_view_model_product_matches() {
    let cube = bounded_cube();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let cams = [
        Camera3::default(),
        Camera3 {
            eye: Vec3::new(6.0, 7.0, 9.0),
            target: Vec3::new(0.0, 0.0, 0.0),
            fov_y: 0.8,
        },
    ];
    let mut checked = (0, 0);
    for cam in &cams {
        let ctx = TnlContext::new(cam, &DEFAULT_LIGHT, SCREEN);
        let view = ctx.projection().0;
        for i in 0..400 {
            let f = i as f32;
            let m = Mat34::from_rotation_scale_translation(
                Vec3::new(0.0, (i % 4) as f32 * core::f32::consts::FRAC_PI_2, 0.0),
                {
                    let k = 0.2 + (i % 7) as f32 * 0.4;
                    Vec3::new(k, k, k)
                },
                Vec3::new(
                    (f * 0.37).sin() * 12.0,
                    (f * 0.11).cos(),
                    (f * 0.23).cos() * 12.0 - 4.0,
                ),
            );
            for params in [
                ModelParams::DEFAULT,
                ModelParams {
                    shading: Shading::Unlit,
                    ..ModelParams::DEFAULT
                },
            ] {
                let a = render3d::rsp_eligible(&ctx, &mesh, &m, &params);
                let b = render3d::rsp_eligible_mv(&ctx, &mesh, &m, &view.mul(&m), &params);
                assert_eq!(a, b, "draw {i}");
                if a.is_some() {
                    checked.0 += 1;
                } else {
                    checked.1 += 1;
                }
            }
        }
    }
    assert!(
        checked.0 > 100 && checked.1 > 100,
        "both answers covered: {checked:?}"
    );
}

/// A pan (eye and target moved by the same exactly representable step)
/// keeps the view's rotation bit for bit, and `RspPan` rebuilds a draw's
/// `view · model` translation, bound center and RSP decision exactly as a
/// full recompute does: a backend may patch remembered draws while the
/// camera pans.
#[test]
fn a_panned_draw_rebuilds_exactly_from_its_pan_parts() {
    let cube = bounded_cube();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let base = Camera3 {
        eye: Vec3::new(0.25, 0.140625, 0.25),
        target: Vec3::new(0.0, 0.0, 0.0),
        fov_y: 0.6,
    };
    let ctx0 = TnlContext::new(&base, &DEFAULT_LIGHT, SCREEN);
    let view0 = ctx0.projection().0;
    let (mut same, mut flips) = (0, 0);
    for step in 1..60 {
        let d = Vec3::new(step as f32 / 512.0, 0.0, -(step as f32) / 1024.0);
        let cam = Camera3 {
            eye: base.eye + d,
            target: base.target + d,
            ..base
        };
        let ctx = TnlContext::new(&cam, &DEFAULT_LIGHT, SCREEN);
        let view = ctx.projection().0;
        for r in 0..3 {
            for c in 0..3 {
                assert_eq!(
                    view.m[r][c].to_bits(),
                    view0.m[r][c].to_bits(),
                    "rotation moved"
                );
            }
        }
        let t = [view.m[0][3], view.m[1][3], view.m[2][3]];
        for i in 0..40 {
            let f = i as f32;
            let k = 1.0 / 128.0;
            let m = Mat34::from_rotation_scale_translation(
                Vec3::new(0.0, (i % 4) as f32 * core::f32::consts::FRAC_PI_2, 0.0),
                Vec3::new(k, k, k),
                Vec3::new((f * 0.37).sin() * 0.12, 0.0, (f * 0.23).cos() * 0.12),
            );
            let pan = render3d::RspPan::new(&view0, &view0.mul(&m), &mesh, &m).unwrap();
            let full = view.mul(&m);
            let mt = pan.translation(t);
            for (r, v) in mt.iter().enumerate() {
                assert_eq!(v.to_bits(), full.m[r][3].to_bits());
            }
            let (c, _) = mesh.bounds().unwrap();
            let fc = full.transform_point(c);
            let pc = pan.center(mt);
            assert_eq!(
                (pc.x.to_bits(), pc.y.to_bits(), pc.z.to_bits()),
                (fc.x.to_bits(), fc.y.to_bits(), fc.z.to_bits())
            );
            let eligible =
                render3d::rsp_eligible_mv(&ctx, &mesh, &m, &full, &ModelParams::DEFAULT).is_some();
            assert_eq!(render3d::rsp_band(&ctx, pc, pan.r2), eligible);
            if eligible {
                same += 1;
            } else {
                flips += 1;
            }
        }
    }
    assert!(same > 100, "{same} eligible, {flips} not");
}

/// A billboard just past the near plane is drawn (screen-aligned, its
/// corners share its depth, so it needs no clipping); one behind it is not.
#[test]
fn billboards_draw_right_up_to_the_near_plane() {
    let cam = Camera3 {
        eye: Vec3::new(0.0, 0.0, 0.0),
        target: Vec3::new(0.0, 0.0, 1.0),
        fov_y: 0.8,
    };
    let p = BillboardParams {
        size: Vec2::new(0.01, 0.01),
        ..BillboardParams::DEFAULT
    };
    let uv = [0.0, 0.0, 1.0, 1.0];
    let near = render3d::NEAR_PLANE;
    for (z, drawn) in [
        (near * 1.2, true),
        (near * 1.9, true),
        (near * 0.9, false),
        (-1.0, false),
    ] {
        let q = render3d::billboard(&cam, &DEFAULT_LIGHT, SCREEN, Vec3::new(0.0, 0.0, z), &p, uv);
        assert_eq!(q.is_some(), drawn, "depth {z}");
        if let Some([a, _]) = q {
            assert!(
                a.z.iter().all(|d| *d > 0.0 && *d < 1.0),
                "depth in range: {:?}",
                a.z
            );
        }
    }
}

/// A draw whose bounding sphere crosses the RSP band but whose vertices
/// are all inside it stays on the RSP (the ground tile under the camera
/// that used to fall back to the CPU); one with a vertex outside still
/// needs clipping.
#[test]
fn a_draw_whose_sphere_crosses_the_band_but_whose_vertices_dont_stays_on_the_rsp() {
    use render3d::RspReject as R;
    let cube = bounded_cube();
    let cube_m = Mesh::from_tmdl(&cube).unwrap();
    let plain = TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN);
    let o = ModelParams::DEFAULT;
    // A ground slab 40 m square, 3 m under the eye, from 2 m in front of
    // it to 42 m: its sphere (radius ~35 m) reaches behind the camera, its
    // corners are all in view of the band.
    let slab = |z: f32| {
        Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::new(40.0, 0.2, 40.0),
            Vec3::new(0.0, -3.0, z),
        )
    };
    let m = slab(22.0);
    let mv = plain.projection().0.mul(&m);
    let (c, rad) = cube_m.bounds().unwrap();
    let r2 = rad * rad * 40.0 * 40.0;
    assert!(
        !render3d::rsp_band(&plain, mv.transform_point(c), r2),
        "the sphere fails"
    );
    assert!(
        render3d::rsp_hull_inside(&plain, &cube_m, &mv),
        "every corner is inside"
    );
    assert_eq!(
        render3d::rsp_check(&plain, &cube_m, &m, &o).map(|_| ()),
        Ok(())
    );
    // Slid toward the camera until corners are behind the near plane: it
    // needs clipping again.
    assert_eq!(
        render3d::rsp_check(&plain, &cube_m, &slab(10.0), &o).map(|_| ()),
        Err(R::NearPlane)
    );
    // And sliding sideways until a corner leaves the band.
    let wide = Mat34::from_rotation_scale_translation(
        Vec3::ZERO,
        Vec3::new(40.0, 0.2, 40.0),
        Vec3::new(14.0, -3.0, 22.0),
    );
    assert_eq!(
        render3d::rsp_check(&plain, &cube_m, &wide, &o).map(|_| ()),
        Err(R::ClipBand)
    );
}

/// `rsp_check` / `rsp_mesh_check` name the rule a draw fails — the first
/// one, in `rsp_eligible`'s order — and agree with `rsp_eligible` on every
/// draw (tools explain the console's work split without changing it).
#[test]
fn rsp_check_names_the_rule_each_draw_fails() {
    use render3d::RspReject as R;
    let cube = bounded_cube();
    let cube_m = Mesh::from_tmdl(&cube).unwrap();
    let tex = textured_cube(SpriteId(1));
    let rig = rigged_cube();
    assert_eq!(render3d::rsp_mesh_check(&cube_m), Ok(()));
    assert_eq!(
        render3d::rsp_mesh_check(&Mesh::from_tmdl(&tex).unwrap()),
        Err(R::TexturedMesh)
    );
    assert_eq!(
        render3d::rsp_mesh_check(&Mesh::from_tmdl(&rig).unwrap()),
        Err(R::Skinned)
    );
    let plain = TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN);
    let fogged = TnlContext::new(
        &Camera3::default(),
        &Light {
            fog: Fog {
                color: Color::BLACK,
                near: 1.0,
                far: 9.0,
            },
            ..DEFAULT_LIGHT
        },
        SCREEN,
    );
    let colored = TnlContext::new(
        &Camera3::default(),
        &Light {
            color: Color::rgb(255, 200, 200),
            ..DEFAULT_LIGHT
        },
        SCREEN,
    );
    let o = ModelParams::DEFAULT;
    let id = Mat34::IDENTITY;
    // A sheared matrix: the overlay can't light it in model space.
    let squash = Mat34 {
        m: [
            [1.0, 0.6, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
        ],
    };
    // A small mesh along +X goes from the fast path straight to culled: the
    // RSP band is wider than the guard band.
    let mut seen = Vec::new();
    for k in 0..400 {
        let m = at(Vec3::new(k as f32 * 0.25, 0.0, 0.0));
        let r = render3d::rsp_check(&plain, &cube_m, &m, &o).map(|_| ());
        if seen.last() != Some(&r) {
            seen.push(r);
        }
    }
    assert_eq!(seen, [Ok(()), Err(R::Offscreen)]);
    // A big one partly in view with corners past the band: it needs
    // clipping.
    let big = Mat34::from_rotation_scale_translation(
        Vec3::ZERO,
        Vec3::new(70.0, 70.0, 70.0),
        Vec3::new(60.0, 0.0, 60.0),
    );
    assert_eq!(
        render3d::rsp_check(&plain, &cube_m, &big, &o).map(|_| ()),
        Err(R::ClipBand)
    );
    let cases: [(&TnlContext, ModelParams, Mat34, Result<(), R>); 12] = [
        (&plain, o, id, Ok(())),
        (&colored, o, id, Err(R::ColoredLight)),
        (&fogged, o, id, Err(R::Fog)),
        (
            &plain,
            ModelParams::tinted(Color::rgba(255, 255, 255, 100)),
            id,
            Err(R::Translucent),
        ),
        (
            &plain,
            ModelParams {
                emissive: Color::rgb(10, 0, 0),
                ..o
            },
            id,
            Err(R::Emissive),
        ),
        (
            &plain,
            ModelParams {
                texture: Some(SpriteId(1)),
                ..o
            },
            id,
            Err(R::Textured),
        ),
        (
            &plain,
            ModelParams {
                double_sided: true,
                ..o
            },
            id,
            Err(R::DoubleSided),
        ),
        (
            &plain,
            ModelParams {
                shading: Shading::Flat,
                ..o
            },
            id,
            Err(R::Flat),
        ),
        (&plain, o, at(Vec3::new(0.0, 0.0, -4.7)), Err(R::NearPlane)),
        (
            &plain,
            o,
            at(Vec3::new(3000.0, 0.0, 0.0)),
            Err(R::Offscreen),
        ),
        (&plain, o, at(Vec3::new(0.0, 0.0, 9000.0)), Err(R::TooFar)),
        (&plain, o, squash, Err(R::NonUniformScale)),
    ];
    for (ctx, params, m, want) in cases {
        let got = render3d::rsp_check(ctx, &cube_m, &m, &params).map(|_| ());
        assert_eq!(got, want, "{params:?} at {m:?}");
        assert_eq!(
            render3d::rsp_check(ctx, &cube_m, &m, &params).ok(),
            render3d::rsp_eligible(ctx, &cube_m, &m, &params),
            "same answer as rsp_eligible"
        );
        // A first failing rule wins over later ones.
        if want == Err(R::Fog) {
            let both = ModelParams {
                shading: Shading::Flat,
                ..params
            };
            assert_eq!(
                render3d::rsp_check(ctx, &cube_m, &m, &both).map(|_| ()),
                Err(R::Fog)
            );
        }
    }
    // Labels for tools are short and distinct.
    let labels = [R::Fog, R::Flat, R::Offscreen, R::NearPlane, R::Skinned].map(R::label);
    let mut uniq = labels.to_vec();
    uniq.sort();
    uniq.dedup();
    assert_eq!(uniq.len(), labels.len());
}
