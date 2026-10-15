//! The `Renderer` trait's provided methods, through a backend that only
//! implements the five required ones (what a new port starts with): every
//! default must be a documented, harmless no-op, and `draw_model_matrix`
//! must decompose into `draw_model` faithfully.

mod common;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Light};
use trino_core::{
    BillboardParams, Caps, Color, FontId, FrameStats, Mat34, Material, ModelId, ModelParams, Rect,
    Renderer, SpriteId, SpriteParams, TextParams, Transform3, Vec2, Vec3,
};

/// The smallest possible backend: records what reaches it.
#[derive(Default)]
struct Minimal {
    frames: u32,
    sprites: Vec<(SpriteId, Vec2)>,
    models: Vec<(ModelId, Transform3, Material)>,
}

impl Renderer for Minimal {
    fn caps(&self) -> &Caps {
        &Caps::N3DS
    }
    fn begin_frame(&mut self, _clear: Color) {}
    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, _: &SpriteParams) {
        self.sprites.push((sprite, pos));
    }
    fn draw_model(&mut self, m: ModelId, t: &Transform3, mat: Material, _: &ModelParams) {
        self.models.push((m, *t, mat));
    }
    fn end_frame(&mut self) {
        self.frames += 1;
    }
}

fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
    (a.x - b.x).abs() < eps && (a.y - b.y).abs() < eps && (a.z - b.z).abs() < eps
}

#[test]
fn provided_renderer_methods_are_harmless_defaults() {
    let mut r = Minimal::default();
    let dyn_r: &mut dyn Renderer = &mut r;
    assert_eq!(dyn_r.caps(), &Caps::N3DS);
    dyn_r.begin_frame(Color::BLACK);
    dyn_r.set_camera(&Camera3::default());
    dyn_r.set_light(&Light {
        ambient: 1.0,
        ..DEFAULT_LIGHT
    });
    dyn_r.set_local_lights(&[trino_core::LocalLight::point(Vec3::ZERO, Color::WHITE, 2.0)]);
    dyn_r.set_wetness(0.5);
    dyn_r.draw_billboard(SpriteId(1), Vec3::ZERO, &BillboardParams::DEFAULT);
    dyn_r.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::WHITE);
    dyn_r.draw_text(FontId(3), Vec2::ZERO, "hello", &TextParams::DEFAULT);
    assert_eq!(dyn_r.measure_text(FontId(3), "hello", 2.0), Vec2::ZERO);
    assert_eq!(dyn_r.second_screen(), None);
    dyn_r.set_screen(1);
    dyn_r.end_frame();
    assert_eq!(dyn_r.stats(), FrameStats::default());
    assert_eq!(dyn_r.read_pixel(0, 0), None);
    // None of the defaults draws through the required methods.
    assert!(r.sprites.is_empty() && r.models.is_empty());
    assert_eq!(r.frames, 1);
}

#[test]
fn draw_model_matrix_default_decomposes_into_draw_model() {
    let mut r = Minimal::default();
    let t = Transform3 {
        position: Vec3::new(1.5, -2.0, 7.25),
        rotation: Vec3::new(0.3, -0.7, 1.1),
        scale: Vec3::new(2.0, 0.5, 3.0),
    };
    r.draw_model_matrix(
        ModelId(9),
        &t.matrix(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    assert_eq!(r.models.len(), 1);
    let (id, got, mat) = r.models[0];
    assert_eq!(id, ModelId(9));
    assert_eq!(mat, Material::VertexLit);
    assert!(close(got.position, t.position, 1e-6), "{got:?}");
    // sin/cos are ~1e-4 approximations, so the basis is unit to ~1e-3.
    assert!(close(got.scale, t.scale, 5e-3), "{got:?}");
    // The core's trig is an approximation (~1e-4 sin/cos, ~5e-3 atan2).
    assert!(close(got.rotation, t.rotation, 1e-2), "{got:?}");
    // Rebuilding the matrix from the decomposition lands every corner of
    // a unit box within a hair of the original.
    for corner in [
        Vec3::ONE,
        Vec3::new(-1.0, 1.0, -1.0),
        Vec3::new(0.5, -1.0, 0.25),
    ] {
        let a = t.matrix().transform_point(corner);
        let b = got.matrix().transform_point(corner);
        assert!(close(a, b, 0.05), "{a:?} vs {b:?}");
    }

    // The identity matrix is the default transform.
    r.draw_model_matrix(
        ModelId(1),
        &Mat34::IDENTITY,
        Material::Sprite,
        &ModelParams::DEFAULT,
    );
    let (_, ident, _) = r.models[1];
    assert!(close(ident.position, Vec3::ZERO, 1e-6));
    // The software sqrt is a 2-step Newton estimate.
    assert!(close(ident.scale, Vec3::ONE, 1e-4));
    assert!(close(ident.rotation, Vec3::ZERO, 1e-6));
}

#[test]
fn param_defaults_and_constructors() {
    assert_eq!(SpriteParams::default(), SpriteParams::DEFAULT);
    assert_eq!(SpriteParams::DEFAULT.scale, Vec2::ONE);
    assert_eq!(SpriteParams::DEFAULT.region, None);
    assert_eq!(TextParams::default(), TextParams::DEFAULT);
    assert_eq!(TextParams::tinted(Color::BLACK).tint, Color::BLACK);
    assert_eq!(TextParams::tinted(Color::BLACK).scale, 1.0);
    assert_eq!(BillboardParams::default(), BillboardParams::DEFAULT);
    assert_eq!(ModelParams::default(), ModelParams::DEFAULT);
    let red = ModelParams::tinted(Color::rgb(255, 0, 0));
    assert_eq!(red.tint, Color::rgb(255, 0, 0));
    assert_eq!(red.emissive, Color::BLACK);
    assert_eq!(Material::default(), Material::Sprite);
    assert_eq!(Transform3::default().matrix(), Mat34::IDENTITY);
    // Handles hash the logical path the same way the bake does.
    assert_eq!(
        SpriteId::from_path("sprites/hero").0,
        trino_core::asset_id("sprites/hero")
    );
    assert_eq!(
        ModelId::from_path("models/a"),
        ModelId::from_path("models/a")
    );
    assert_ne!(
        ModelId::from_path("models/a"),
        ModelId::from_path("models/b")
    );
    assert_eq!(
        FontId::from_path("fonts/ui").0,
        trino_core::asset_id("fonts/ui")
    );
}

/// The recording test double (shared by the other suites) reports what it
/// counted through `stats()` once the frame ends, like real backends.
#[test]
fn recorder_stats_cover_the_last_finished_frame() {
    let mut r = common::Recorder::new();
    r.add_mesh(ModelId(1), common::bounded_cube());
    r.begin_frame(Color::BLACK);
    r.draw_sprite(SpriteId(2), Vec2::ZERO, &SpriteParams::DEFAULT);
    r.fill_rect(Rect::new(1.0, 1.0, 2.0, 2.0), Color::WHITE);
    r.draw_model(
        ModelId(1),
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    assert_eq!(r.stats(), FrameStats::default(), "not finished yet");
    r.end_frame();
    let s = r.stats();
    assert_eq!(s.sprites, 2);
    assert_eq!(s.models, 1);
    // A cube seen face-on: one face (two triangles) toward the camera.
    assert_eq!(s.tris, 2);
    assert_eq!(s.tnl.emitted, 2);
    assert_eq!(s.tnl.tris_in, 12);
}
