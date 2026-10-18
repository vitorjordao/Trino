//! The PC renderer end to end: a synthetic manifest is baked by the real
//! asset pipeline, loaded, uploaded and drawn through the `Renderer` trait;
//! the frame is read back and probed.
//!
//! GPU-gated like the goldens: no adapter = skip, unless
//! `TRINO_REQUIRE_GPU` is set. The probes sit in the middle of flat areas
//! with tolerances, so they hold on any driver (lavapipe included).

mod common;

use common::*;
use trino_core::render3d::{Camera3, Fog, Light};
use trino_core::{
    BillboardParams, Color, Mat34, Material, ModelParams, Rect, Renderer, Screen, Shading,
    SpriteParams, TextAlign, TextParams, Transform3, Vec2, Vec3,
};
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

const CLEAR: Color = Color::rgb(0, 0, 0);

fn read(r: &mut PcRenderer, x: u32, y: u32) -> [u8; 4] {
    let c = r.read_pixel(x, y).expect("inside the frame");
    [c.r, c.g, c.b, c.a]
}

/// Sprites (plain, scaled + tinted, flipped, region, rotated, translucent),
/// rectangles and bitmap text from baked assets, plus the per-frame stats.
#[test]
fn baked_sprites_rects_and_text_draw_on_the_pc_renderer() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    let mut platform = PcPlatform::new(&SimProfile::Native.caps());
    upload(&assets, &mut r, Some(&mut platform));
    assert_eq!(r.internal_size(), (640, 480));
    assert_eq!(r.caps().screen_width, 640);
    assert!(!r.n64_look(), "the native profile has no console look");

    let text = TextParams {
        scale: 2.0,
        tint: Color::rgb(255, 255, 0),
        align: TextAlign::Left,
    };
    r.begin_frame(CLEAR);
    // 1. Plain 8x8 red sprite at (10, 10).
    r.draw_sprite(sprite(RED), Vec2::new(10.0, 10.0), &SpriteParams::DEFAULT);
    // 2. White ×4 tinted blue → a 32x32 blue square at (40, 10).
    r.draw_sprite(
        sprite(WHITE),
        Vec2::new(40.0, 10.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            tint: Color::rgb(0, 0, 255),
            ..SpriteParams::DEFAULT
        },
    );
    // 3. Split (green | blue) ×4, flipped: blue | green at (100, 10).
    r.draw_sprite(
        sprite(SPLIT),
        Vec2::new(100.0, 10.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            flip_x: true,
            ..SpriteParams::DEFAULT
        },
    );
    // 4. Only the right (blue) half of split, ×4 at (160, 10): 16x32.
    r.draw_sprite(
        sprite(SPLIT),
        Vec2::new(160.0, 10.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            region: Some(Rect::new(4.0, 0.0, 4.0, 8.0)),
            ..SpriteParams::DEFAULT
        },
    );
    // 5. Split ×4 turned a quarter: the halves become top and bottom.
    r.draw_sprite(
        sprite(SPLIT),
        Vec2::new(220.0, 10.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            rotation: core::f32::consts::FRAC_PI_2,
            ..SpriteParams::DEFAULT
        },
    );
    // 6. Half-transparent white over the black clear.
    r.draw_sprite(
        sprite(WHITE),
        Vec2::new(280.0, 10.0),
        &SpriteParams {
            scale: Vec2::new(4.0, 4.0),
            tint: Color::rgba(255, 255, 255, 128),
            ..SpriteParams::DEFAULT
        },
    );
    // Unknown handles are skipped (no draw, no stat).
    r.draw_sprite(
        trino_core::SpriteId(0xDEAD),
        Vec2::ZERO,
        &SpriteParams::DEFAULT,
    );
    r.draw_text(trino_core::FontId(0xBEEF), Vec2::ZERO, "AB", &text);
    // 7. A filled rectangle.
    r.fill_rect(Rect::new(200.0, 200.0, 20.0, 10.0), Color::rgb(10, 200, 30));
    // 8. "AB" at scale 2: two 6x12 yellow glyph blocks, 8 px apart.
    r.draw_text(font(), Vec2::new(300.0, 300.0), "AB", &text);
    // 9. Right-aligned "A" ending at x = 400 (an 8 px advance).
    r.draw_text(
        font(),
        Vec2::new(400.0, 340.0),
        "A",
        &TextParams {
            align: TextAlign::Right,
            ..text
        },
    );
    r.end_frame();

    let frame = r.read_offscreen();
    assert_eq!(frame.len(), 640 * 480 * 4);
    let at = |x, y| px(&frame, 640, x, y);
    assert_near(at(13, 13), [255, 0, 0], 8, "plain sprite");
    assert_near(at(5, 5), [0, 0, 0], 8, "clear color around it");
    assert_near(at(56, 26), [0, 0, 255], 8, "scaled + tinted");
    assert_near(at(74, 26), [0, 0, 0], 8, "scale stops at 32 px");
    assert_near(at(106, 26), [0, 0, 255], 8, "flipped: blue on the left");
    assert_near(at(126, 26), [0, 255, 0], 8, "flipped: green on the right");
    assert_near(at(168, 26), [0, 0, 255], 8, "region: right half only");
    assert_near(at(182, 26), [0, 0, 0], 8, "region is 16 px wide");
    let (top, bottom) = (at(236, 14), at(236, 38));
    assert!(
        top != bottom
            && [top, bottom]
                .iter()
                .all(|p| p[0] < 20 && p[1].max(p[2]) > 200),
        "rotated: halves are top and bottom ({top:?} / {bottom:?})"
    );
    let half = at(296, 26);
    assert!(
        (100..=156).contains(&half[0]) && half[0].abs_diff(half[1]) < 6,
        "alpha blend ≈ 50% gray, got {half:?}"
    );
    assert_near(at(210, 205), [10, 200, 30], 8, "fill_rect");
    assert_near(at(303, 306), [255, 255, 0], 8, "glyph A");
    assert_near(at(311, 306), [255, 255, 0], 8, "glyph B");
    assert_near(at(307, 306), [0, 0, 0], 8, "gap between glyphs");
    assert_near(at(395, 346), [255, 255, 0], 8, "right-aligned glyph");
    assert_near(at(401, 346), [0, 0, 0], 8, "nothing right of the anchor");

    // The trait's readback agrees with the bulk readback.
    assert_eq!(read(&mut r, 13, 13), at(13, 13));
    assert_eq!(r.read_pixel(640, 0), None);
    assert_eq!(r.read_pixel(0, 480), None);

    // measure_text through the renderer = the baked table's metrics:
    // advance 3 + 1 spacing, line height = the 6 px cell.
    assert_eq!(r.measure_text(font(), "AB", 2.0), Vec2::new(16.0, 12.0));
    assert_eq!(r.measure_text(font(), "A\nAB", 1.0), Vec2::new(8.0, 12.0));
    assert_eq!(
        r.measure_text(font(), "Z", 1.0),
        Vec2::new(4.0, 6.0),
        "missing glyphs measure as `?`"
    );
    assert_eq!(
        r.measure_text(trino_core::FontId(1), "AB", 1.0),
        Vec2::ZERO,
        "unknown font"
    );

    // 6 sprites + 1 rect + 3 glyphs; the unknown handles count nothing.
    let stats = r.stats();
    assert_eq!(stats.sprites, 10);
    assert_eq!((stats.models, stats.tris), (0, 0));
    assert!(stats.max_texture_bytes >= 8 * 8 * 4);

    // The blob reached the platform unchanged.
    use trino_core::Platform;
    assert_eq!(
        platform.blob(trino_core::BlobId::from_path(BLOB)),
        Some(BLOB_BYTES)
    );
}

/// Fixed camera looking down +Z from z = -5 (the default camera).
fn camera() -> Camera3 {
    Camera3::default()
}

/// A light traveling +Z: it hits the faces that look at the camera.
fn light(color: Color, fog: Fog) -> Light {
    Light {
        dir: Vec3::new(0.0, 0.0, 1.0),
        ambient: 0.0,
        color,
        ambient_color: Color::BLACK,
        fog,
    }
}

/// 3D from baked fake objects: lighting, shading modes, tint, emissive,
/// fog, z-buffered occlusion across batches, `draw_model_matrix`, a
/// textured model, billboards and the T&L stats.
#[test]
fn baked_models_light_fog_depth_textures_and_billboards() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);
    let (cx, cy) = (320, 240);
    let unlit = |tint| ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::tinted(tint)
    };
    let frame = |r: &mut PcRenderer, l: Light, draw: &dyn Fn(&mut PcRenderer)| {
        r.begin_frame(CLEAR);
        r.set_camera(&camera());
        r.set_light(&l);
        draw(r);
        r.end_frame();
    };

    // Gouraud: a red light on a white box, head-on → red face.
    frame(&mut r, light(Color::rgb(255, 0, 0), Fog::NONE), &|r| {
        r.draw_model(
            model(BOX),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        )
    });
    let lit = read(&mut r, cx, cy);
    assert!(
        lit[0] > 200 && lit[1] < 30 && lit[2] < 30,
        "red key light: {lit:?}"
    );
    assert_near(read(&mut r, 20, 20), [0, 0, 0], 4, "clear around the box");
    let stats = r.stats();
    assert_eq!(stats.models, 1);
    if r.hw_tnl() {
        // The GPU T&L path took it (issue #51): its work is not the CPU's.
        assert_eq!(
            (
                stats.tnl.pc_gpu_draws,
                stats.tnl.pc_gpu_tris,
                stats.tnl.verts
            ),
            (1, 12, 0),
            "a box is 12 source triangles: {stats:?}"
        );
        assert_eq!(stats.tris, 12);
    } else {
        assert!(stats.tris >= 2 && stats.tnl.verts >= 8, "{stats:?}");
        assert!(stats.tnl.tris_in >= 12, "a box is 12 source triangles");
    }

    // Tint multiplies before lighting: green tint under a red light = black;
    // emissive is added after lighting.
    frame(&mut r, light(Color::rgb(255, 0, 0), Fog::NONE), &|r| {
        r.draw_model(
            model(BOX),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                emissive: Color::rgb(0, 0, 200),
                ..ModelParams::tinted(Color::rgb(0, 255, 0))
            },
        )
    });
    assert_near(
        read(&mut r, cx, cy),
        [0, 0, 200],
        12,
        "tint × light + emissive",
    );

    // Unlit ignores the light; Flat still lights.
    frame(&mut r, light(Color::rgb(255, 0, 0), Fog::NONE), &|r| {
        r.draw_model(
            model(BOX),
            &Transform3::default(),
            Material::VertexLit,
            &unlit(Color::WHITE),
        )
    });
    assert_near(read(&mut r, cx, cy), [255, 255, 255], 6, "unlit");
    frame(&mut r, light(Color::rgb(0, 255, 0), Fog::NONE), &|r| {
        r.draw_model(
            model(BOX),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                shading: Shading::Flat,
                ..ModelParams::DEFAULT
            },
        )
    });
    let flat = read(&mut r, cx, cy);
    assert!(
        flat[1] > 200 && flat[0] < 30,
        "flat shading lit green: {flat:?}"
    );

    // Fog that is full well before the box: the box turns the fog color.
    let fog = Fog {
        color: Color::rgb(255, 0, 255),
        near: 0.1,
        far: 1.0,
    };
    frame(&mut r, light(Color::WHITE, fog), &|r| {
        r.draw_model(
            model(BOX),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        )
    });
    assert_near(read(&mut r, cx, cy), [255, 0, 255], 12, "fully fogged");

    // Occlusion through the depth buffer: the near red box is drawn first,
    // then (in a separate batch, after a flushing sprite call) a large far
    // blue box. Red must win in the middle; blue shows around it.
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        r.draw_model(
            model(BOX),
            &Transform3::default(),
            Material::VertexLit,
            &unlit(Color::rgb(255, 0, 0)),
        );
        // Flushes the model batch (unknown sprite: nothing drawn).
        r.draw_sprite(
            trino_core::SpriteId(0xDEAD),
            Vec2::ZERO,
            &SpriteParams::DEFAULT,
        );
        r.draw_model(
            model(BOX),
            &Transform3 {
                position: Vec3::new(0.0, 0.0, 3.0),
                scale: Vec3::new(4.0, 4.0, 1.0),
                ..Transform3::default()
            },
            Material::VertexLit,
            &unlit(Color::rgb(0, 0, 255)),
        );
    });
    assert_near(read(&mut r, cx, cy), [255, 0, 0], 6, "near box in front");
    assert_near(
        read(&mut r, cx + 80, cy),
        [0, 0, 255],
        6,
        "far box around it",
    );
    assert_eq!(r.stats().models, 2);

    // draw_model_matrix with the same matrix draws the same pixels as
    // draw_model with the transform.
    let t = Transform3 {
        position: Vec3::new(-1.0, 0.5, 0.0),
        rotation: Vec3::new(0.3, 0.6, 0.0),
        scale: Vec3::new(0.8, 0.8, 0.8),
    };
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        r.draw_model(model(BOX), &t, Material::VertexLit, &ModelParams::DEFAULT)
    });
    let by_transform = r.read_offscreen();
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        let m: Mat34 = t.matrix();
        r.draw_model_matrix(model(BOX), &m, Material::VertexLit, &ModelParams::DEFAULT)
    });
    assert!(
        by_transform == r.read_offscreen(),
        "draw_model == draw_model_matrix"
    );
    assert!(by_transform.chunks(4).any(|p| p[0] > 50), "something drew");

    // Textured fake object: the baked default texture (solid green) shows;
    // a per-draw texture override (the red sprite, 8x8) replaces it.
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        r.draw_model(
            model(TBOX),
            &Transform3::default(),
            Material::VertexLit,
            &unlit(Color::WHITE),
        )
    });
    assert_near(read(&mut r, cx, cy), [0, 255, 0], 10, "baked texture");
    assert!(r.stats().max_texture_bytes >= 16 * 16 * 4);
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        r.draw_model(
            model(TBOX),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                texture: Some(sprite(RED)),
                ..unlit(Color::WHITE)
            },
        )
    });
    assert_near(read(&mut r, cx, cy), [255, 0, 0], 10, "texture override");

    // Billboards: a camera-facing quad textured with the sprite and tinted;
    // fog applies to them too.
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        r.draw_billboard(
            sprite(WHITE),
            Vec3::ZERO,
            &BillboardParams {
                size: Vec2::new(1.0, 1.0),
                tint: Color::rgb(255, 128, 0),
                ..BillboardParams::DEFAULT
            },
        );
        r.draw_billboard(
            trino_core::SpriteId(0xDEAD),
            Vec3::ZERO,
            &BillboardParams::DEFAULT,
        );
    });
    assert_near(read(&mut r, cx, cy), [255, 128, 0], 8, "billboard");
    assert_near(read(&mut r, cx + 100, cy), [0, 0, 0], 4, "billboard size");
    assert_eq!(r.stats().sprites, 1, "the unknown billboard is skipped");
    frame(&mut r, light(Color::WHITE, fog), &|r| {
        r.draw_billboard(sprite(WHITE), Vec3::ZERO, &BillboardParams::DEFAULT)
    });
    assert_near(read(&mut r, cx, cy), [255, 0, 255], 12, "fogged billboard");

    // Unknown models are skipped without counting.
    frame(&mut r, light(Color::WHITE, Fog::NONE), &|r| {
        r.draw_model(
            trino_core::ModelId(0xDEAD),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        )
    });
    assert_eq!(r.stats().models, 0);
    assert_near(read(&mut r, cx, cy), [0, 0, 0], 4, "nothing drawn");
}

/// Resizing the internal picture, the device-sharing constructor, the
/// tooling accessors and the window mapping without a window.
#[test]
fn internal_size_shared_device_and_tooling_accessors() {
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    assert_eq!(r.internal_size(), (320, 240));
    assert!(!r.adapter_info().is_empty());
    eprintln!("adapter: {}", r.adapter_info());
    r.set_internal_size(200, 100);
    assert_eq!(r.internal_size(), (200, 100));
    assert_eq!((r.caps().screen_width, r.caps().screen_height), (200, 100));
    r.set_internal_size(200, 100); // unchanged: a no-op
    r.set_internal_size(1, 1);
    assert_eq!(r.internal_size(), (16, 16), "clamped to 16 px");
    r.set_internal_size(64, 48);
    r.set_n64_look(false);
    r.begin_frame(Color::rgb(30, 60, 90));
    r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::rgb(250, 250, 250));
    r.end_frame();
    r.wait_gpu();
    let frame = r.read_offscreen();
    assert_eq!(frame.len(), 64 * 48 * 4);
    assert_near(
        px(&frame, 64, 4, 4),
        [250, 250, 250],
        4,
        "rect after resize",
    );
    assert_near(
        px(&frame, 64, 40, 40),
        [30, 60, 90],
        2,
        "clear after resize",
    );
    let _ = r.offscreen_view();
    // Headless: no window, so resize is a no-op and window pixels map to
    // nothing.
    r.resize(800, 600);
    assert_eq!(r.window_to_game(10.0, 10.0), None);
    assert_eq!(r.window_to_pointer(10.0, 10.0), None);
    // A window of exactly 2× the picture shows it pixel-doubled.
    let preview = r.present_preview((128, 96));
    assert_eq!(preview.len(), 128 * 96 * 4);
    assert_near(px(&preview, 128, 8, 8), [250, 250, 250], 4, "doubled rect");
    // A wider window letterboxes with black bars.
    let wide = r.present_preview((256, 96));
    assert_near(px(&wide, 256, 10, 50), [0, 0, 0], 0, "side bar");
    assert_near(px(&wide, 256, 128, 50), [30, 60, 90], 2, "picture");
    // The same pointer events as a window would get: without a window the
    // cursor maps outside the picture.
    let mut input = trino_platform_pc::PcInput::new();
    use trino_core::Input;
    use winit::dpi::PhysicalPosition;
    use winit::event::WindowEvent as E;
    use winit::event::{DeviceId, ElementState, MouseButton, MouseScrollDelta, TouchPhase};
    let device_id = DeviceId::dummy();
    input.handle_pointer(Some(Vec2::new(1.0, 1.0)));
    assert!(input.handle_window_event(
        &E::CursorMoved {
            device_id,
            position: PhysicalPosition::new(5.0, 5.0)
        },
        &r
    ));
    assert_eq!(input.poll().pointer, None, "no window: off the picture");
    input.handle_pointer(Some(Vec2::new(1.0, 1.0)));
    assert!(input.handle_window_event(&E::CursorLeft { device_id }, &r));
    assert_eq!(input.poll().pointer, None);
    assert!(input.handle_window_event(
        &E::MouseInput {
            device_id,
            state: ElementState::Pressed,
            button: MouseButton::Middle
        },
        &r
    ));
    assert!(input.handle_window_event(
        &E::MouseWheel {
            device_id,
            delta: MouseScrollDelta::LineDelta(0.0, 2.0),
            phase: TouchPhase::Moved
        },
        &r
    ));
    assert!(input.handle_window_event(
        &E::MouseWheel {
            device_id,
            delta: MouseScrollDelta::PixelDelta(PhysicalPosition::new(0.0, 80.0)),
            phase: TouchPhase::Moved
        },
        &r
    ));
    assert!(
        !input.handle_window_event(&E::Focused(true), &r),
        "not input"
    );
    let s = input.poll();
    assert!(s.pointer_down(trino_core::PointerButton::Middle));
    assert_eq!(s.wheel, 4.0, "2 lines + 80 px (40 px per notch)");

    // A renderer built on a caller's device (the editor's path) renders
    // like the headless one.
    let instance = wgpu::Instance::new(wgpu::InstanceDescriptor::new_without_display_handle());
    let adapter = pollster::block_on(instance.request_adapter(&Default::default())).unwrap();
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        required_limits: wgpu::Limits::downlevel_defaults(),
        ..Default::default()
    }))
    .unwrap();
    let mut shared = PcRenderer::with_device(device, queue, SimProfile::N3ds);
    assert_eq!(shared.internal_size(), (400, 240));
    assert_eq!(shared.adapter_info(), "editor device");
    assert_eq!(shared.second_screen(), Some(Vec2::new(320.0, 240.0)));
    shared.begin_frame(Color::rgb(0, 128, 0));
    shared.end_frame();
    assert_near(read(&mut shared, 200, 120), [0, 128, 0], 2, "shared device");
}

/// The 3DS profile: a second (touch) screen for 2D, rendered and read back
/// separately and presented under the main one.
#[test]
fn n3ds_second_screen_renders_reads_back_and_presents_stacked() {
    let Some(mut r) = headless(SimProfile::N3ds) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);
    assert_eq!(r.internal_size(), (400, 240));
    assert_eq!(r.second_screen(), Some(Vec2::new(320.0, 240.0)));

    r.begin_frame(Color::rgb(200, 0, 0));
    r.fill_rect(Rect::new(0.0, 0.0, 10.0, 10.0), Color::rgb(0, 0, 250));
    r.set_screen(1);
    r.fill_rect(Rect::new(100.0, 100.0, 40.0, 40.0), Color::rgb(0, 250, 0));
    r.draw_sprite(sprite(RED), Vec2::new(10.0, 10.0), &SpriteParams::DEFAULT);
    r.set_screen(1); // already there: no-op
    r.end_frame();

    let top = r.read_offscreen();
    let bottom = r.read_second_screen();
    assert_eq!(bottom.len(), 320 * 240 * 4);
    assert_near(px(&top, 400, 5, 5), [0, 0, 250], 8, "main screen rect");
    assert_near(px(&top, 400, 200, 120), [200, 0, 0], 4, "main clear");
    assert_near(
        px(&top, 400, 120, 120),
        [200, 0, 0],
        4,
        "screen-1 draws stay off the main",
    );
    assert_near(
        px(&bottom, 320, 120, 120),
        [0, 250, 0],
        8,
        "second screen rect",
    );
    assert_near(
        px(&bottom, 320, 13, 13),
        [255, 0, 0],
        12,
        "second screen sprite",
    );
    assert_near(
        px(&bottom, 320, 300, 200),
        [16, 18, 28],
        2,
        "touch-screen background",
    );
    assert_eq!(r.stats().sprites, 3);

    // Both screens stacked at 2×: 400x480 into 800x960.
    let preview = r.present_preview((800, 960));
    assert_near(px(&preview, 800, 10, 10), [0, 0, 250], 8, "main, top-left");
    assert_near(
        px(&preview, 800, 80 + 240, 480 + 240),
        [0, 250, 0],
        8,
        "second, centered below",
    );
    assert_near(
        px(&preview, 800, 20, 700),
        [0, 0, 0],
        0,
        "bars beside the narrower screen",
    );

    // A frame that never visits screen 1 still clears it.
    r.begin_frame(Color::rgb(0, 0, 0));
    r.end_frame();
    let bottom = r.read_second_screen();
    assert_near(
        px(&bottom, 320, 120, 120),
        [16, 18, 28],
        2,
        "cleared second screen",
    );

    // The typed API: top first, then the bottom, no way back; the
    // read-back hook reads either screen.
    r.begin_frame(Color::rgb(0, 0, 90));
    assert_eq!(r.screen(), Screen::Top);
    assert_eq!(r.screen_size(Screen::Top), Some(Vec2::new(400.0, 240.0)));
    assert_eq!(r.screen_size(Screen::Bottom), Some(Vec2::new(320.0, 240.0)));
    assert!(r.on_screen(Screen::Top));
    r.fill_rect(Rect::new(0.0, 0.0, 400.0, 20.0), Color::rgb(250, 250, 0));
    assert!(r.on_screen(Screen::Bottom));
    r.fill_rect(Rect::new(0.0, 0.0, 320.0, 20.0), Color::rgb(0, 250, 250));
    assert!(!r.on_screen(Screen::Top), "the frame already left the top");
    assert_eq!(r.screen(), Screen::Bottom);
    r.fill_rect(Rect::new(0.0, 220.0, 320.0, 20.0), Color::rgb(0, 250, 250));
    r.end_frame();
    let at = |r: &mut PcRenderer, s: Screen, x: u32, y: u32| {
        let c = r.read_screen_pixel(s, x, y).unwrap();
        [c.r, c.g, c.b, c.a]
    };
    assert_near(at(&mut r, Screen::Top, 5, 5), [250, 250, 0], 8, "top strip");
    assert_near(
        at(&mut r, Screen::Top, 5, 230),
        [0, 0, 90],
        4,
        "refused switch",
    );
    assert_near(at(&mut r, Screen::Bottom, 5, 5), [0, 250, 250], 8, "bottom");
    assert_near(
        at(&mut r, Screen::Bottom, 5, 230),
        [0, 250, 250],
        8,
        "stays",
    );
    assert_eq!(r.read_screen_pixel(Screen::Bottom, 320, 0), None);
    assert_eq!(r.stats().sprites, 3, "both screens counted");

    // Single-screen profiles have none.
    if let Some(mut n64) = headless(SimProfile::N64) {
        assert_eq!(n64.second_screen(), None);
        assert!(n64.read_second_screen().is_empty());
        assert_eq!(n64.screen_size(Screen::Bottom), None);
        n64.begin_frame(Color::BLACK);
        assert!(!n64.on_screen(Screen::Bottom));
        assert_eq!(n64.screen(), Screen::Top);
        n64.end_frame();
        assert_eq!(n64.read_screen_pixel(Screen::Bottom, 0, 0), None);
    }
}

/// The N64 profile's console look (16-bit clear, VI dedither switch) and
/// strict mode's budget checks on content that would not fit the console.
#[test]
fn n64_look_and_strict_mode_budgets() {
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);

    // The look is on by default: the clear keeps 5 bits per channel.
    assert!(r.n64_look() && r.vi_dedither());
    let clear = Color::rgb(100, 150, 200);
    r.begin_frame(clear);
    r.end_frame();
    assert_near(read(&mut r, 160, 120), [99, 148, 206], 0, "RGBA5551 clear");
    r.set_vi_dedither(false);
    assert!(!r.vi_dedither());
    r.begin_frame(clear);
    r.end_frame();
    assert_near(
        read(&mut r, 160, 120),
        [99, 148, 206],
        0,
        "flat: no VI change",
    );
    r.set_n64_look(false);
    r.set_n64_look(false); // unchanged: no-op
    r.begin_frame(clear);
    r.end_frame();
    assert_near(
        read(&mut r, 160, 120),
        [100, 150, 200],
        0,
        "look off: exact",
    );

    r.set_strict(true);
    let panics = |r: &mut PcRenderer, f: &dyn Fn(&mut PcRenderer)| -> String {
        let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(r)))
            .expect_err("strict mode must reject this");
        err.downcast_ref::<String>().cloned().unwrap_or_default()
    };
    let draw = |r: &mut PcRenderer,
                texture: Option<trino_core::SpriteId>,
                double_sided: bool,
                n: usize| {
        r.begin_frame(Color::BLACK);
        r.set_camera(&camera());
        for _ in 0..n {
            r.draw_model(
                model(match (texture, double_sided) {
                    (Some(_), _) => TBOX,
                    (None, true) => BALL,
                    (None, false) => BOX,
                }),
                &Transform3::default(),
                Material::VertexLit,
                &ModelParams {
                    texture,
                    double_sided,
                    ..ModelParams::DEFAULT
                },
            );
        }
        r.end_frame();
    };
    // Within budget: a textured box and a sphere draw fine.
    draw(&mut r, Some(sprite(TEX)), false, 1);
    draw(&mut r, None, false, 1);

    // More sprite draws than the N64 profile allows per frame.
    let msg = panics(&mut r, &|r| {
        r.begin_frame(Color::BLACK);
        for _ in 0..513 {
            r.fill_rect(Rect::new(0.0, 0.0, 1.0, 1.0), Color::WHITE);
        }
        r.end_frame();
    });
    assert!(msg.contains("sprites this frame"), "{msg}");

    // More rasterized triangles than the profile's cap (double-sided
    // spheres of ~2300 triangles each).
    let msg = panics(&mut r, &|r| draw(r, None, true, 2));
    assert!(msg.contains("3D triangles"), "{msg}");
    // One single-sided sphere fits the triangle cap but not the N64's
    // estimated 3D time (the calibrated cost model).
    let msg = panics(&mut r, &|r| {
        r.begin_frame(Color::BLACK);
        r.set_camera(&camera());
        r.draw_model(
            model(BALL),
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        r.end_frame();
    });
    assert!(msg.contains("ceiling"), "{msg}");

    // A model texture that is not power-of-two cannot tile on a console.
    r.upload_sprite(trino_core::SpriteId(77), 12, 12, &solid(12, 12, [9; 4]));
    let msg = panics(&mut r, &|r| {
        draw(r, Some(trino_core::SpriteId(77)), false, 1)
    });
    assert!(msg.contains("power-of-two"), "{msg}");

    // A POT texture larger than TMEM (64x64 at 16 bpp = 8 KB > 4 KB).
    r.upload_sprite(trino_core::SpriteId(78), 64, 64, &solid(64, 64, [9; 4]));
    let msg = panics(&mut r, &|r| {
        draw(r, Some(trino_core::SpriteId(78)), false, 1)
    });
    assert!(msg.contains("3D texture") && msg.contains("TMEM"), "{msg}");
    // The same oversized texture on a billboard (clamped, untiled) is
    // still checked against texture memory.
    let msg = panics(&mut r, &|r| {
        r.begin_frame(Color::BLACK);
        r.draw_billboard(
            trino_core::SpriteId(78),
            Vec3::ZERO,
            &BillboardParams::DEFAULT,
        );
        r.end_frame();
    });
    assert!(msg.contains("3D texture"), "{msg}");

    // Broken blobs are rejected at upload with the handle in the message.
    let msg = panics(&mut r, &|r| {
        r.upload_mesh(trino_core::ModelId(5), b"nope".to_vec())
    });
    assert!(msg.contains("invalid TMDL"), "{msg}");
    let msg = panics(&mut r, &|r| {
        r.upload_font(trino_core::FontId(5), b"nope".to_vec())
    });
    assert!(msg.contains("invalid TFNT"), "{msg}");

    // More models loaded than the console's RAM: 4 MB on the base N64,
    // 8 MB with the Expansion Pak (issue #54, ADR 0020).
    let ball = assets
        .models
        .iter()
        .find(|m| trino_core::ModelId(m.id) == model(BALL))
        .unwrap()
        .tmdl
        .clone();
    for i in 0..(5 << 20) / ball.len() + 1 {
        r.upload_mesh(trino_core::ModelId(0x7000_0000 + i as u32), ball.clone());
    }
    let empty = |r: &mut PcRenderer| {
        r.begin_frame(Color::BLACK);
        r.end_frame();
    };
    let msg = panics(&mut r, &|r| empty(r));
    assert!(msg.contains("of RAM on the Base tier"), "{msg}");
    r.set_hw_tier(trino_core::HwTier::Expanded);
    empty(&mut r);
    r.set_hw_tier(trino_core::HwTier::Base);

    // The renderer keeps working after the rejected frames.
    r.set_strict(false);
    r.begin_frame(Color::rgb(0, 0, 248));
    r.end_frame();
    assert_near(read(&mut r, 10, 10), [0, 0, 255], 8, "recovers");
}

/// Frames bigger than the renderer's initial GPU buffers (1024 sprite
/// instances, 1024 triangles) still draw everything; one model batch mixes
/// textures; a billboard behind the camera is skipped.
#[test]
fn large_frames_grow_buffers_and_batches_mix_textures() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);

    // 1600 rectangles: a 40x40 grid of 4x4 cells, the last one white.
    r.begin_frame(CLEAR);
    for i in 0..1600u32 {
        let (x, y) = ((i % 40) * 4, (i / 40) * 4);
        let c = if i == 1599 {
            Color::WHITE
        } else {
            Color::rgb(0, (i % 200) as u8 + 40, 0)
        };
        r.fill_rect(Rect::new(x as f32, y as f32, 4.0, 4.0), c);
    }
    r.end_frame();
    assert_eq!(r.stats().sprites, 1600);
    assert_near(
        read(&mut r, 157, 157),
        [255, 255, 255],
        4,
        "the 1600th rect",
    );
    assert_near(read(&mut r, 1, 1), [0, 40, 0], 4, "the first rect");

    // A double-sided 48-segment sphere is ~2300 triangles in one draw;
    // a textured box in the same batch on each side, default texture
    // (green) on the left, a per-draw override (red) on the right.
    r.begin_frame(CLEAR);
    r.set_camera(&camera());
    r.set_light(&light(Color::WHITE, Fog::NONE));
    let unlit = |texture| ModelParams {
        shading: Shading::Unlit,
        double_sided: true,
        texture,
        ..ModelParams::DEFAULT
    };
    r.draw_model(
        model(BALL),
        &Transform3 {
            scale: Vec3::new(0.5, 0.5, 0.5),
            ..Transform3::default()
        },
        Material::VertexLit,
        &unlit(None),
    );
    for (x, texture) in [(-1.8, None), (1.8, Some(sprite(RED)))] {
        r.draw_model(
            model(TBOX),
            &Transform3 {
                position: Vec3::new(x, 0.0, 0.0),
                scale: Vec3::new(0.8, 0.8, 0.8),
                ..Transform3::default()
            },
            Material::VertexLit,
            &unlit(texture),
        );
    }
    // Behind the eye: no quad, no stat.
    r.draw_billboard(
        sprite(WHITE),
        Vec3::new(0.0, 0.0, -10.0),
        &BillboardParams::DEFAULT,
    );
    r.end_frame();
    let stats = r.stats();
    assert!(stats.tris > 2_000, "{stats:?}");
    assert_eq!((stats.models, stats.sprites), (3, 0));
    assert_near(read(&mut r, 320, 240), [255, 255, 255], 6, "sphere");
    // Camera at z = -5, fov 60°: x = ±1.8 lands ~150 px off center.
    assert_near(
        read(&mut r, 320 - 150, 240),
        [0, 255, 0],
        10,
        "default texture",
    );
    assert_near(
        read(&mut r, 320 + 150, 240),
        [255, 0, 0],
        10,
        "override texture",
    );
}

/// The frame's GPU calls and the pipeline changes between them
/// (`PcFrameTimes::draw_calls`, `pipeline_switches`): what a browser pays
/// for in JavaScript, and what the editor's Web Game view shows (issue #47).
#[test]
fn frame_times_count_calls_and_pipeline_switches() {
    let Some(mut r) = headless(SimProfile::Web) else {
        return;
    };
    let dir = tempfile::tempdir().unwrap();
    let assets = bake_project(dir.path());
    upload(&assets, &mut r, None);
    // Sprites only: one pipeline, no change.
    r.begin_frame(CLEAR);
    r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::WHITE);
    r.end_frame();
    let t = r.frame_times();
    assert!(t.draw_calls >= 1, "{t:?}");
    assert_eq!(t.pipeline_switches, 0, "{t:?}");
    // A 3D model, then a sprite over it: 3D, then back to 2D.
    r.begin_frame(CLEAR);
    r.set_camera(&camera());
    r.set_light(&light(Color::WHITE, Fog::NONE));
    r.set_hw_tnl(false);
    r.draw_model(
        model(TBOX),
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    r.fill_rect(Rect::new(0.0, 0.0, 8.0, 8.0), Color::WHITE);
    r.end_frame();
    let t = r.frame_times();
    assert!(t.draw_calls >= 2, "{t:?}");
    assert!(t.pipeline_switches >= 1, "{t:?}");
    assert!(t.pipeline_switches < t.draw_calls, "{t:?}");
}

/// Color slots and a per-draw palette (TMDL `SLOT`, `ModelParams::palette`):
/// one white card baked with two slot regions, drawn with two different
/// palettes and without one. The two halves take their palette entries;
/// without a palette the card stays white.
#[test]
fn a_palette_recolors_slot_regions_per_draw() {
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    // Two quads side by side (the seam has its own vertices, so the
    // regions don't blend): left in slot 1, right in slot 2.
    let mut card = trino_asset_pipeline::MeshData::default();
    for (x0, slot) in [(-1.0f32, 1u8), (0.0, 2)] {
        let base = card.positions.len() as u32;
        for (x, y) in [(0.0, -1.0), (1.0, -1.0), (1.0, 1.0), (0.0, 1.0)] {
            card.positions.push([x0 + x, y, 0.0]);
            card.normals.push([0.0, 0.0, -1.0]);
            card.colors.push([255; 4]);
            card.slots.push(slot);
        }
        card.indices
            .extend([base, base + 1, base + 2, base, base + 2, base + 3]);
    }
    let id = trino_core::ModelId::from_path("models/slotted_card");
    r.upload_mesh(id, card.encode().unwrap());
    let draw = |r: &mut PcRenderer, palette| {
        r.begin_frame(CLEAR);
        r.set_camera(&camera());
        r.set_light(&light(Color::WHITE, Fog::NONE));
        r.draw_model(
            id,
            &Transform3::default(),
            Material::VertexLit,
            &ModelParams {
                shading: Shading::Unlit,
                double_sided: true,
                palette,
                ..ModelParams::DEFAULT
            },
        );
        r.end_frame();
        let mut sides = [read(r, 280, 240), read(r, 360, 240)];
        sides.sort();
        sides
    };
    let (red, blue, green) = (
        Color::rgb(220, 30, 30),
        Color::rgb(30, 40, 210),
        Color::rgb(20, 200, 60),
    );
    let sorted = |a: Color, b: Color| {
        let mut s = [[a.r, a.g, a.b, 255], [b.r, b.g, b.b, 255]];
        s.sort();
        s
    };
    for (one, two) in [(red, blue), (green, red)] {
        let got = draw(&mut r, Some([one, two, Color::WHITE, Color::WHITE]));
        let want = sorted(one, two);
        for k in 0..2 {
            assert_near(
                got[k],
                [want[k][0], want[k][1], want[k][2]],
                6,
                "slot color",
            );
        }
    }
    for side in draw(&mut r, None) {
        assert_near(side, [255, 255, 255], 6, "no palette: baked white");
    }
}
