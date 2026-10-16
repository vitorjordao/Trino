//! Local lights (point and spot) through the public T&L API: the
//! per-vertex tier adds them to the vertex colors of the draws they reach
//! and nothing else; with none, the output is bit-identical; the N64's RSP
//! test sends lit draws to the CPU; the per-pixel tier's surface path
//! emits the same screen triangles plus each vertex's surface.

mod common;

use common::*;
use trino_core::render3d::{
    self, Camera3, DEFAULT_LIGHT, Mesh, ScreenTri, SurfaceVert, TnlContext, TnlScratch, TnlStats,
};
use trino_core::{Color, LocalLight, Mat34, ModelParams, RspReject, Shading, Vec3};

fn ctx(lights: &[LocalLight]) -> TnlContext {
    TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN).with_local_lights(lights)
}

fn draw(
    ctx: &TnlContext,
    blob: &[u8],
    model: &Mat34,
    params: &ModelParams,
) -> (Vec<ScreenTri>, TnlStats) {
    let mesh = Mesh::from_tmdl(blob).unwrap();
    let mut out = Vec::new();
    let mut scratch = TnlScratch::new();
    let s = render3d::tessellate_with(ctx, &mut scratch, &mesh, model, params, &mut |t| {
        out.push(t)
    });
    (out, s)
}

fn at(p: Vec3) -> Mat34 {
    Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, p)
}

fn same(a: &[ScreenTri], b: &[ScreenTri]) -> bool {
    a.len() == b.len()
        && a.iter().zip(b).all(|(x, y)| {
            x.colors == y.colors && x.pts == y.pts && x.z == y.z && x.depth == y.depth
        })
}

/// A red point light between the camera (z = -5) and the cube's front face.
fn red_lamp() -> LocalLight {
    LocalLight::point(Vec3::new(0.0, 0.0, -1.5), Color::rgb(255, 0, 0), 2.5).with_intensity(0.3)
}

#[test]
fn no_lights_and_lights_out_of_reach_change_nothing() {
    let cube = bounded_cube();
    let (base, s0) = draw(&ctx(&[]), &cube, &Mat34::IDENTITY, &ModelParams::DEFAULT);
    let far = LocalLight::point(Vec3::new(9.0, 0.0, 0.0), Color::WHITE, 2.0);
    let (lit, s1) = draw(&ctx(&[far]), &cube, &Mat34::IDENTITY, &ModelParams::DEFAULT);
    assert!(same(&base, &lit), "a light that can't reach the cube");
    assert_eq!(s0, s1, "same T&L work");
    // A light with no color or radius is dropped.
    let dark = LocalLight::point(Vec3::ZERO, Color::BLACK, 3.0);
    let empty = LocalLight::point(Vec3::ZERO, Color::WHITE, 0.0);
    assert_eq!(ctx(&[dark, empty]).local_light_count(), 0);
}

#[test]
fn a_point_light_adds_its_color_to_the_faces_it_sees() {
    let cube = bounded_cube();
    let (base, _) = draw(&ctx(&[]), &cube, &Mat34::IDENTITY, &ModelParams::DEFAULT);
    let (lit, s) = draw(
        &ctx(&[red_lamp()]),
        &cube,
        &Mat34::IDENTITY,
        &ModelParams::DEFAULT,
    );
    assert_eq!(s.emitted, 2, "the front face only");
    for (b, l) in base.iter().zip(&lit) {
        assert_eq!(b.pts, l.pts, "geometry untouched");
        for k in 0..3 {
            assert!(
                l.colors[k].r > b.colors[k].r + 10,
                "red added: {:?} vs {:?}",
                l.colors[k],
                b.colors[k]
            );
            assert_eq!(l.colors[k].g, b.colors[k].g, "only red");
            assert_eq!(l.colors[k].b, b.colors[k].b);
        }
    }
    // Twice the intensity: brighter still.
    let (hot, _) = draw(
        &ctx(&[red_lamp().with_intensity(0.6)]),
        &cube,
        &Mat34::IDENTITY,
        &ModelParams::DEFAULT,
    );
    assert!(hot[0].colors[0].r > lit[0].colors[0].r);
}

#[test]
fn faces_turned_away_from_the_light_stay_dark() {
    // The light sits behind the cube (z = +1.5): the front face can't see it.
    let behind = LocalLight::point(Vec3::new(0.0, 0.0, 1.5), Color::rgb(255, 0, 0), 3.0);
    let cube = bounded_cube();
    let (base, _) = draw(&ctx(&[]), &cube, &Mat34::IDENTITY, &ModelParams::DEFAULT);
    let (lit, _) = draw(
        &ctx(&[behind]),
        &cube,
        &Mat34::IDENTITY,
        &ModelParams::DEFAULT,
    );
    assert!(same(&base, &lit));
}

#[test]
fn a_spot_lights_only_inside_its_cone() {
    let cube = bounded_cube();
    let (base, _) = draw(&ctx(&[]), &cube, &Mat34::IDENTITY, &ModelParams::DEFAULT);
    let toward = LocalLight::spot(
        Vec3::new(0.0, 0.0, -1.5),
        Vec3::new(0.0, 0.0, 1.0),
        0.6,
        0.9,
        Color::rgb(0, 0, 255),
        3.0,
    );
    let (lit, _) = draw(
        &ctx(&[toward]),
        &cube,
        &Mat34::IDENTITY,
        &ModelParams::DEFAULT,
    );
    assert!(
        lit[0].colors[0].b > base[0].colors[0].b + 20,
        "inside the cone"
    );
    let aside = LocalLight::spot(
        Vec3::new(0.0, 0.0, -1.5),
        Vec3::new(1.0, 0.0, 0.0),
        0.2,
        0.3,
        Color::rgb(0, 0, 255),
        3.0,
    );
    let (missed, _) = draw(
        &ctx(&[aside]),
        &cube,
        &Mat34::IDENTITY,
        &ModelParams::DEFAULT,
    );
    assert!(same(&base, &missed), "pointing away");
}

#[test]
fn unlit_draws_ignore_local_lights_and_flat_draws_get_them() {
    let cube = bounded_cube();
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let (a, _) = draw(&ctx(&[]), &cube, &Mat34::IDENTITY, &unlit);
    let (b, _) = draw(&ctx(&[red_lamp()]), &cube, &Mat34::IDENTITY, &unlit);
    assert!(same(&a, &b));
    let flat = ModelParams {
        shading: Shading::Flat,
        ..ModelParams::DEFAULT
    };
    let (a, _) = draw(&ctx(&[]), &cube, &Mat34::IDENTITY, &flat);
    let (b, _) = draw(&ctx(&[red_lamp()]), &cube, &Mat34::IDENTITY, &flat);
    assert!(b[0].colors[0].r > a[0].colors[0].r + 20);
}

#[test]
fn the_rsp_refuses_lit_draws_and_keeps_the_rest() {
    let cube = bounded_cube();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let lit = ctx(&[red_lamp()]);
    let near = Mat34::IDENTITY;
    let far = at(Vec3::new(1.6, 0.0, 3.0));
    assert_eq!(
        render3d::rsp_check(&lit, &mesh, &near, &ModelParams::DEFAULT).map(|_| ()),
        Err(RspReject::LocalLight)
    );
    assert!(render3d::rsp_check(&lit, &mesh, &far, &ModelParams::DEFAULT).is_ok());
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    assert!(
        render3d::rsp_check(&lit, &mesh, &near, &unlit).is_ok(),
        "unlit: no light to add"
    );
    assert!(render3d::rsp_check(&ctx(&[]), &mesh, &near, &ModelParams::DEFAULT).is_ok());
}

#[test]
fn the_surface_path_emits_the_same_triangles_plus_each_vertex_surface() {
    let cube = bounded_cube();
    let mesh = Mesh::from_tmdl(&cube).unwrap();
    let model = Mat34::from_rotation_scale_translation(
        Vec3::new(0.0, 0.4, 0.0),
        Vec3::new(2.0, 2.0, 2.0),
        Vec3::new(0.5, 0.0, 1.0),
    );
    let params = ModelParams {
        tint: Color::rgba(255, 128, 255, 255),
        gloss: 200,
        ..ModelParams::DEFAULT
    };
    let c = ctx(&[]);
    let (plain, s0) = draw(&c, &cube, &model, &params);
    let mut tris = Vec::new();
    let mut surf: Vec<[SurfaceVert; 3]> = Vec::new();
    let mut scratch = trino_core::SurfaceScratch::new();
    let s1 = render3d::tessellate_surface(&c, &mut scratch, &mesh, &model, &params, &mut |t, v| {
        tris.push(t);
        surf.push(*v);
    });
    assert_eq!(s0, s1);
    assert!(same(&plain, &tris));
    assert_eq!(surf.len(), tris.len());
    for v in surf.iter().flatten() {
        assert!(
            (v.normal.dot(v.normal) - 1.0).abs() < 1e-3,
            "unit world normal"
        );
        // On the scaled cube's surface: every coordinate at most 1 from
        // its center (2 m cube) and one of them exactly on a face.
        let d = v.pos - Vec3::new(0.5, 0.0, 1.0);
        assert!(d.dot(d) <= 3.0 + 1e-3, "{d:?}");
        // The unlit color is a face's vertex color × the tint.
        let k = 128.0 / 255.0;
        assert!(
            CUBE_FACES
                .iter()
                .any(|(_, c)| v.base == Color::rgba(c[0], (c[1] as f32 * k) as u8, c[2], 255)),
            "{:?}",
            v.base
        );
    }
}
