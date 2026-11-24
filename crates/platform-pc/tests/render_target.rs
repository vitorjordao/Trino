//! Render targets ([`Renderer::begin_target`], Bongo-Kart #184) on the PC
//! renderer: a target's picture is byte for byte what a screen of its size
//! draws, on the GPU path and the CPU path; the screen samples it as a
//! sprite and as a 3D texture the frame it is drawn and after, and follows
//! a resize; the order rule (targets first) and the caps refuse, counted,
//! and strict mode says why; the console profiles draw none.
//! GPU-gated like the goldens.

use trino_asset_pipeline::primitive::{Pivot, Primitive, Shape};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Fog};
use trino_core::{
    Color, DynVertex, DynamicMesh, Mat34, Material, ModelId, ModelParams, Rect, RenderTarget,
    Renderer, Shading, SpriteParams, Transform3, Vec2, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const BOX: ModelId = ModelId(31);
const BALL: ModelId = ModelId(32);
const FLOOR: ModelId = ModelId(33);
const MIRROR: RenderTarget = RenderTarget::new("targets/mirror", 160, 90);

/// One GPU test at a time (parallel headless devices crash some drivers).
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile, size: (u32, u32)) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(mut r) => {
            if profile == SimProfile::Native {
                r.set_internal_size(size.0, size.1);
            }
            r.set_hw_tnl(true);
            Some(r)
        }
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            None
        }
    }
}

fn prim(shape: Shape, color: [u8; 4], segments: u32) -> Vec<u8> {
    Primitive {
        shape,
        color,
        pivot: Pivot::Bottom,
        segments,
        ..Primitive::default()
    }
    .build()
    .encode()
    .unwrap()
}

fn upload_scene(r: &mut PcRenderer) {
    r.upload_mesh(
        BALL,
        prim(Shape::Sphere { radius: 0.6 }, [90, 170, 230, 255], 12),
    );
    r.upload_mesh(
        BOX,
        prim(
            Shape::Box {
                size: [1.0, 0.8, 1.2],
            },
            [200, 140, 80, 255],
            2,
        ),
    );
    r.upload_mesh(
        FLOOR,
        prim(
            Shape::Box {
                size: [14.0, 0.2, 14.0],
            },
            [120, 150, 90, 255],
            1,
        ),
    );
}

fn camera(behind: bool) -> Camera3 {
    let z = if behind { 7.0 } else { -7.0 };
    Camera3 {
        eye: Vec3::new(0.0, 3.0, z),
        target: Vec3::new(0.0, 0.4, 0.0),
        fov_y: 60f32.to_radians(),
    }
}

fn at(x: f32, z: f32, yaw: f32) -> Transform3 {
    Transform3 {
        position: Vec3::new(x, 0.0, z),
        rotation: Vec3::new(0.0, yaw, 0.0),
        ..Transform3::default()
    }
}

/// The scene a mirror and a screen both draw: a floor, boxes and balls in
/// fog, and a HUD rectangle.
fn draw_scene(r: &mut dyn Renderer, behind: bool) {
    r.set_camera(&camera(behind));
    let mut light = DEFAULT_LIGHT;
    light.fog = Fog {
        color: Color::rgb(140, 160, 190),
        near: 5.0,
        far: 16.0,
    };
    r.set_light(&light);
    let p = ModelParams::DEFAULT;
    r.draw_model(FLOOR, &at(0.0, 0.0, 0.0), Material::VertexLit, &p);
    for k in 0..5 {
        let x = k as f32 * 1.6 - 3.2;
        r.draw_model(BOX, &at(x, 1.5, 0.4 * k as f32), Material::VertexLit, &p);
        r.draw_model(BALL, &at(x, -1.5, 0.0), Material::VertexLit, &p);
    }
    r.fill_rect(
        Rect::new(4.0, 4.0, 30.0, 8.0),
        Color::rgba(250, 220, 60, 200),
    );
}

const CLEAR: Color = Color::rgb(40, 60, 90);

/// What the screen shows at its own size, and what a target of that size
/// shows of the same draws: the same bytes, on both T&L paths.
#[test]
fn a_target_draws_what_a_screen_of_its_size_draws() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native, (160, 90)) else {
        return;
    };
    upload_scene(&mut r);
    for gpu_path in [true, false] {
        r.set_hw_tnl(gpu_path);
        r.begin_frame(CLEAR);
        draw_scene(&mut r, false);
        r.end_frame();
        let screen = r.read_offscreen();

        r.begin_frame(Color::BLACK);
        assert!(r.begin_target(&MIRROR, CLEAR));
        draw_scene(&mut r, false);
        r.end_target();
        r.end_frame();
        let target = r.read_target(&MIRROR).expect("a drawn target reads back");
        assert_eq!(target.len(), screen.len());
        let differ = target.iter().zip(&screen).filter(|(a, b)| a != b).count();
        assert_eq!(differ, 0, "GPU path {gpu_path}: {differ} bytes differ");
        let stats = r.stats();
        assert_eq!((stats.targets, stats.targets_rejected), (1, 0));
        assert!(stats.models >= 11 && stats.tris > 0, "{stats:?}");
    }
}

fn pixel(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 3] {
    let i = ((y * width + x) * 4) as usize;
    [rgba[i], rgba[i + 1], rgba[i + 2]]
}

fn near(got: [u8; 3], want: Color) -> bool {
    let w = [want.r, want.g, want.b];
    got.iter()
        .zip(w)
        .all(|(a, b)| (*a as i32 - b as i32).abs() <= 3)
}

/// A 32×16 target, its left half red and its right blue.
fn two_tone(r: &mut PcRenderer, t: &RenderTarget, left: Color, right: Color) {
    assert!(r.begin_target(t, right));
    r.fill_rect(
        Rect::new(0.0, 0.0, f32::from(t.width) / 2.0, f32::from(t.height)),
        left,
    );
    r.end_target();
}

/// A screen-filling quad textured with `t` (its UVs over the whole
/// texture, the left half of the screen showing the texture's left half).
fn textured_wall(r: &mut PcRenderer, t: &RenderTarget) {
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.0),
        target: Vec3::ZERO,
        fov_y: 60f32.to_radians(),
    });
    r.set_light(&DEFAULT_LIGHT);
    let n = Vec3::new(0.0, 0.0, -1.0);
    let v = [
        (-4.0, 3.0, 0.0, 0.0),
        (4.0, 3.0, 1.0, 0.0),
        (4.0, -3.0, 1.0, 1.0),
        (-4.0, -3.0, 0.0, 1.0),
    ]
    .map(|(x, y, u, w)| DynVertex {
        uv: Vec2::new(u, w),
        ..DynVertex::new(Vec3::new(x, y, 0.0), n, Color::WHITE)
    });
    r.draw_mesh(
        &DynamicMesh::new(&v, &[0, 1, 2, 0, 2, 3]).textured(t.sprite),
        &Mat34::IDENTITY,
        &ModelParams {
            shading: Shading::Unlit,
            double_sided: true,
            ..ModelParams::DEFAULT
        },
    );
}

#[test]
fn the_screen_samples_a_target_and_it_keeps_its_picture() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native, (160, 90)) else {
        return;
    };
    let (red, blue, green, white) = (
        Color::rgb(220, 30, 30),
        Color::rgb(30, 30, 220),
        Color::rgb(30, 200, 30),
        Color::rgb(240, 240, 240),
    );
    let t = RenderTarget::new("targets/screen", 32, 16);
    for gpu_path in [true, false] {
        r.set_hw_tnl(gpu_path);
        // Drawn and shown the same frame: a sprite at 2x, top left, over a
        // textured wall filling the screen.
        r.begin_frame(Color::BLACK);
        two_tone(&mut r, &t, red, blue);
        textured_wall(&mut r, &t);
        r.draw_sprite(
            t.sprite,
            Vec2::new(100.0, 4.0),
            &SpriteParams {
                scale: Vec2::new(2.0, 2.0),
                ..SpriteParams::DEFAULT
            },
        );
        r.end_frame();
        let f = r.read_offscreen();
        assert!(
            near(pixel(&f, 160, 104, 10), red),
            "{:?}",
            pixel(&f, 160, 104, 10)
        );
        assert!(near(pixel(&f, 160, 156, 30), blue));
        assert!(
            near(pixel(&f, 160, 20, 60), red),
            "{:?}",
            pixel(&f, 160, 20, 60)
        );
        assert!(near(pixel(&f, 160, 140, 60), blue));

        // Not drawn this frame: the picture stays.
        r.begin_frame(Color::BLACK);
        textured_wall(&mut r, &t);
        r.end_frame();
        let f = r.read_offscreen();
        assert!(near(pixel(&f, 160, 20, 60), red) && near(pixel(&f, 160, 140, 60), blue));
        assert_eq!(r.stats().targets, 0);

        // Resized: a new texture, and the quad's cached 3D binding follows it.
        let small = t.sized(16, 16);
        r.begin_frame(Color::BLACK);
        two_tone(&mut r, &small, green, white);
        textured_wall(&mut r, &small);
        r.end_frame();
        let f = r.read_offscreen();
        assert!(
            near(pixel(&f, 160, 20, 60), green),
            "{:?}",
            pixel(&f, 160, 20, 60)
        );
        assert!(near(pixel(&f, 160, 140, 60), white));
        assert_eq!(r.read_target(&small).unwrap().len(), 16 * 16 * 4);
        // Back to its first size for the other path.
        r.begin_frame(Color::BLACK);
        two_tone(&mut r, &t, red, blue);
        r.end_frame();
    }
}

#[test]
fn targets_come_first_and_fit_the_caps() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native, (160, 90)) else {
        return;
    };
    let caps = *r.caps();
    assert_eq!((caps.max_target_texels, caps.max_targets), (1 << 20, 4));
    r.begin_frame(Color::BLACK);
    // Too big, empty, then four that fit and a fifth over the frame's count.
    assert!(!r.begin_target(&MIRROR.sized(2048, 1024), CLEAR));
    assert!(!r.begin_target(&MIRROR.sized(0, 90), CLEAR));
    for k in 0..4 {
        let t = RenderTarget::new("targets/many", 8, 8 + k);
        assert!(r.begin_target(&t, CLEAR), "target {k}");
        // Another one while this one is open is refused.
        assert!(!r.begin_target(&MIRROR, CLEAR));
        r.end_target();
    }
    assert!(!r.begin_target(&MIRROR, CLEAR), "a fifth target");
    r.end_target(); // none open: harmless
    r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::WHITE);
    r.end_frame();
    assert_eq!(
        (r.stats().targets, r.stats().targets_rejected),
        (4, 7),
        "{:?}",
        r.stats()
    );

    // After a screen draw: refused (strict mode off), the screen unharmed.
    r.begin_frame(Color::BLACK);
    r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::WHITE);
    assert!(!r.begin_target(&MIRROR, CLEAR));
    r.end_frame();
    assert_eq!((r.stats().targets, r.stats().targets_rejected), (0, 1));
    assert_eq!(pixel(&r.read_offscreen(), 160, 2, 2), [255, 255, 255]);

    // A target left open is ended by end_frame, and the next frame starts
    // on the screen.
    r.begin_frame(Color::BLACK);
    assert!(r.begin_target(&MIRROR, Color::rgb(200, 0, 0)));
    r.end_frame();
    assert_eq!(r.stats().targets, 1);
    assert_eq!(
        pixel(&r.read_target(&MIRROR).unwrap(), 160, 5, 5),
        [200, 0, 0]
    );
    r.begin_frame(Color::rgb(0, 0, 200));
    r.end_frame();
    assert_eq!(pixel(&r.read_offscreen(), 160, 5, 5), [0, 0, 200]);
}

/// Strict mode says why a target after a screen draw drew nothing.
#[test]
#[should_panic(expected = "render targets first")]
fn strict_mode_says_targets_come_first() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::Native, (160, 90)) else {
        panic!("skipped: no adapter (render targets first)");
    };
    r.set_strict(true);
    r.begin_frame(Color::BLACK);
    r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::WHITE);
    r.begin_target(&MIRROR, CLEAR);
}

#[test]
fn console_profiles_draw_no_targets() {
    let _gpu = gpu();
    for profile in [SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile, (0, 0)) else {
            return;
        };
        r.set_strict(true);
        assert_eq!(r.caps().max_target_texels, 0);
        r.begin_frame(Color::BLACK);
        assert!(!r.begin_target(&RenderTarget::new("targets/tiny", 8, 8), CLEAR));
        r.end_target();
        r.end_frame();
        assert_eq!((r.stats().targets, r.stats().targets_rejected), (0, 1));
        assert_eq!(r.read_target(&MIRROR), None);
    }
}
