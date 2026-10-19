//! Engine feature checks shared by every target's self-test: the PC test
//! suite runs them against the wgpu backend, the N64/3DS apps run them in
//! ares/Azahar (`cargo xtask test n64|3ds`) — same code, same expectations.
//!
//! Each check exercises one engine service through the portable traits
//! only, so a pass on a console proves the backend wiring (shim, asset
//! bake, loader) end to end.

use trino_core::HwTier;
use trino_core::math3d::{cos, sin};
use trino_core::{
    BakedScene, BillboardParams, BlobId, Camera3, Cells, Color, DEFAULT_LIGHT, DetailTexture,
    Effect, Emitter, EntityStore, FontId, Impostor, InputState, Instance, Light, LocalLight,
    Material, ModelId, ModelParams, Palette, ParticleSystem, Platform, QualityPreset,
    QualitySettings, Rect, Renderer, Screen, Shading, ShadowCast, ShadowPlane, ShadowSettings,
    ShadowTier, SpriteId, SpriteParams, TexFilter, TextBuf, TextParams, Transform3, Vec2, Vec3,
};

/// The HUD font (also what the checks measure).
pub const FONT: FontId = FontId::from_path("fonts/trino5x7");
/// `assets/shared/data/selftest.txt`, declared as `[blobs.selftest]`.
pub const BLOB: BlobId = BlobId::from_path("blobs/selftest");
pub const BLOB_CONTENT: &[u8] = b"trino-blob-ok\n";
/// A 1x1 card textured with four colored quadrants (`[models.selftest_card]`).
pub const CARD: ModelId = ModelId::from_path("models/selftest_card");

/// Run every engine check; `Err` names the first failure.
pub fn engine_checks(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<(), &'static str> {
    // Progress markers make an emulator hang/crash point obvious in logs.
    platform.log("TRINO_STEP:text\n");
    text(renderer)?;
    platform.log("TRINO_STEP:blob\n");
    blob(platform)?;
    platform.log("TRINO_STEP:save\n");
    save(platform)?;
    platform.log("TRINO_STEP:textured_3d\n");
    if textured_3d(renderer)? {
        platform.log("TRINO_STEP:textured_3d probed 4 quadrants\n");
    } else {
        platform.log("TRINO_STEP:textured_3d skipped (no read_pixel)\n");
    }
    platform.log("TRINO_STEP:texture filter and detail\n");
    if texture_detail(renderer)? {
        platform.log("TRINO_STEP:texture filter and detail probed\n");
    }
    platform.log("TRINO_STEP:texture formats\n");
    if texture_formats(renderer, platform)? {
        platform.log("TRINO_STEP:texture formats probed 9 formats\n");
    }
    platform.log("TRINO_STEP:billboards\n");
    if billboards(renderer)? {
        platform.log("TRINO_STEP:billboards probed\n");
    }
    platform.log("TRINO_STEP:occlusion\n");
    if occlusion(renderer)? {
        platform.log("TRINO_STEP:occlusion probed\n");
    }
    platform.log("TRINO_STEP:fill rect\n");
    if fill_rect(renderer)? {
        platform.log("TRINO_STEP:fill rect probed\n");
    }
    platform.log("TRINO_STEP:ui layout\n");
    if ui_layout(renderer, platform)? {
        platform.log("TRINO_STEP:ui layout probed focus, pad and touch\n");
    }
    platform.log("TRINO_STEP:i18n\n");
    if i18n(renderer, platform)? {
        platform.log("TRINO_STEP:i18n probed 3 languages\n");
    }
    platform.log("TRINO_STEP:dual screen\n");
    if dual_screen(renderer)? {
        platform.log("TRINO_STEP:dual screen probed both screens\n");
    } else if renderer.screen_size(Screen::Bottom).is_none() {
        platform.log("TRINO_STEP:dual screen single screen (collapsed)\n");
    }
    platform.log("TRINO_STEP:smooth skin\n");
    smooth_skin(renderer)?;
    platform.log("TRINO_STEP:palette\n");
    if palette(renderer)? {
        platform.log("TRINO_STEP:palette probed 3 slots\n");
    }
    platform.log("TRINO_STEP:instancing\n");
    if instancing(renderer)? {
        platform.log("TRINO_STEP:instancing probed\n");
    }
    platform.log("TRINO_STEP:local lights\n");
    if local_lights(renderer)? {
        platform.log("TRINO_STEP:local lights probed\n");
    }
    platform.log("TRINO_STEP:ambient occlusion\n");
    if ambient_occlusion(renderer)? {
        platform.log("TRINO_STEP:ambient occlusion probed\n");
    }
    platform.log("TRINO_STEP:shadows\n");
    if shadows(renderer, platform)? {
        platform.log("TRINO_STEP:shadows probed\n");
    }
    platform.log("TRINO_STEP:water\n");
    if water(renderer)? {
        platform.log("TRINO_STEP:water probed\n");
    }
    platform.log("TRINO_STEP:impostor\n");
    if impostor(renderer, platform)? {
        platform.log("TRINO_STEP:impostor probed\n");
    }
    platform.log("TRINO_STEP:particles\n");
    particles(renderer, platform)?;
    platform.log("TRINO_STEP:portals\n");
    portals(renderer, platform)?;
    platform.log("TRINO_STEP:accessibility\n");
    if accessibility(renderer, platform)? {
        platform
            .log("TRINO_STEP:accessibility probed palette swap, outline, captions and contrast\n");
    }
    platform.log("TRINO_STEP:hw tier\n");
    let tier = hw_tier(renderer, platform)?;
    log_tier(platform, &tier);
    platform.log("TRINO_STEP:quality\n");
    let q = quality(renderer, platform)?;
    log_quality(platform, &q);
    platform.log("TRINO_STEP:engine ok\n");
    Ok(())
}

/// The quadrants in 16-bit color (no palette: they share TMEM with a
/// detail layer on the N64) and the detail layer, white left, gray right.
pub const QUADRANTS16: SpriteId = SpriteId::from_path("sprites/selftest_quadrants16");
pub const DETAIL: SpriteId = SpriteId::from_path("sprites/selftest_detail");

/// Texture filtering and the detail layer: the quadrant card magnified
/// (~11 px per texel), probed 2.6 px left of the red/green texel edge —
/// pure red with `TexFilter::Point`, a red/green blend with `Bilinear` (the
/// RDP's 3-point on the N64, whose texel centers sit half a texel off). Then the detail layer: the right half of the
/// card (gray detail) at half the color, the left half (white) at full.
/// `Ok(false)` where `read_pixel` is unsupported.
pub fn texture_detail(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let draw = |r: &mut dyn Renderer, params: &ModelParams| {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            r.set_camera(&camera);
            r.set_light(&DEFAULT_LIGHT);
            r.draw_model(CARD, &Transform3::default(), Material::VertexLit, params);
            r.end_frame();
        }
    };
    // Front face at z = -0.05; planar UVs u = x + 0.5, v = 0.5 - y.
    let probe = |r: &mut dyn Renderer, u: f32, v: f32| {
        let p = view.transform_point(Vec3::new(u - 0.5, 0.5 - v, -0.05));
        let x = screen.x * 0.5 + p.x * focal / p.z;
        let y = screen.y * 0.5 - p.y * focal / p.z;
        r.read_pixel(x as u32, y as u32)
    };
    let base = ModelParams {
        shading: Shading::Unlit,
        texture: Some(QUADRANTS16),
        ..ModelParams::DEFAULT
    };
    // Filter: 0.24 texel left of the edge between texels 7 (red) and 8.
    for (filter, mixed) in [(TexFilter::Point, false), (TexFilter::Bilinear, true)] {
        draw(renderer, &ModelParams { filter, ..base });
        let Some(c) = probe(renderer, 0.485, 0.25) else {
            return Ok(false);
        };
        // The RDP's bilinear centers its texels half a texel off the PC's
        // (ares: 74, 189, 0 here, the PC: ~180, 70, 0): both blend.
        if mixed && (c.r < 40 || c.g < 40) {
            return Err("texture filter: Bilinear did not blend the texel edge");
        }
        if !mixed && (c.r < 150 || c.g > 30) {
            return Err("texture filter: Point blended the texel edge");
        }
    }
    // Detail: halves the right column, keeps the left.
    draw(
        renderer,
        &ModelParams {
            filter: TexFilter::Point,
            detail: Some(DetailTexture::new(DETAIL, 0)),
            ..base
        },
    );
    let near = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 40;
    for (u, v, want) in [
        (0.25, 0.25, [255, 0, 0]),
        (0.75, 0.25, [0, 128, 0]),
        (0.25, 0.75, [0, 0, 255]),
        (0.75, 0.75, [128, 128, 128]),
    ] {
        let Some(c) = probe(renderer, u, v) else {
            return Ok(false);
        };
        if !(near(c.r, want[0]) && near(c.g, want[1]) && near(c.b, want[2])) {
            return Err("texture detail: the detail layer was not multiplied in");
        }
    }
    Ok(true)
}

/// One card per 3DS texture format (`[sprites.selftest_fmt_*]`, issue #52),
/// in manifest order with its TL, TR, BL, BR quadrants (`None` =
/// transparent). The N64 bakes every card as RGBA5551, which stores the same
/// colors, so each target expects this table.
pub const FORMAT_CARDS: [(SpriteId, [Option<[u8; 3]>; 4]); 9] = {
    const R: Option<[u8; 3]> = Some([255, 0, 0]);
    const G: Option<[u8; 3]> = Some([0, 255, 0]);
    const B: Option<[u8; 3]> = Some([0, 0, 255]);
    const W: Option<[u8; 3]> = Some([255, 255, 255]);
    const GRAY: Option<[u8; 3]> = Some([136, 136, 136]);
    const DARK: Option<[u8; 3]> = Some([68, 68, 68]);
    const BLACK: Option<[u8; 3]> = Some([0, 0, 0]);
    const CLEAR: Option<[u8; 3]> = None;
    [
        (
            SpriteId::from_path("sprites/selftest_fmt_rgba8"),
            [R, G, B, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_rgb565"),
            [R, G, B, W],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_rgba5551"),
            [R, G, B, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_rgba4"),
            [R, G, B, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_la8"),
            [W, GRAY, DARK, CLEAR],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_l8"),
            [W, GRAY, DARK, BLACK],
        ),
        // A8 is a white mask on every target (the 3DS shim tints it).
        (
            SpriteId::from_path("sprites/selftest_fmt_a8"),
            [W, CLEAR, CLEAR, W],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_etc1"),
            [R, G, B, W],
        ),
        (
            SpriteId::from_path("sprites/selftest_fmt_etc1a4"),
            [R, G, B, CLEAR],
        ),
    ]
};

/// [`FORMAT_CARDS`]' 3DS formats, for the failure log.
const FORMAT_NAMES: [&str; 9] = [
    "RGBA8", "RGB565", "RGBA5551", "RGBA4", "LA8", "L8", "A8", "ETC1", "ETC1A4",
];

/// Log a failed probe: format, 2D or 3D, quadrant, the color read and the
/// one expected.
fn log_probe(
    platform: &mut dyn Platform,
    i: usize,
    kind: &str,
    q: usize,
    got: Color,
    want: [u8; 3],
) {
    use core::fmt::Write;
    let mut line: TextBuf<96> = TextBuf::new();
    let _ = writeln!(
        line,
        "TRINO_STEP:texture formats {} {kind} q{q} got {},{},{} want {},{},{}",
        FORMAT_NAMES[i], got.r, got.g, got.b, want[0], want[1], want[2]
    );
    platform.log(line.as_str());
}

/// The texture-formats frame's background (a transparent texel shows it).
const FORMATS_BG: [u8; 3] = [96, 0, 96];
/// Its 3D cards: 0.45 m, 0.5 m apart, centered, 0.8 m below the axis.
const FORMAT_CARD_SCALE: f32 = 0.45;

fn formats_camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 0.0, -3.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    }
}

/// 2D cards: 32 px, from x = 4, 35 px apart (fits the N64's 320).
fn format_sprite_at(i: usize) -> Vec2 {
    Vec2::new(4.0 + 35.0 * i as f32, 8.0)
}

fn format_card_at(i: usize) -> Vec3 {
    Vec3::new((i as f32 - 4.0) * 0.5, -0.8, 0.0)
}

/// The photo-like card of the 3DS golden: master, ETC1, RGBA4.
pub const FORMAT_PHOTOS: [SpriteId; 3] = [
    SpriteId::from_path("sprites/selftest_photo"),
    SpriteId::from_path("sprites/selftest_photo_etc1"),
    SpriteId::from_path("sprites/selftest_photo_rgba4"),
];

/// The texture-formats frame (three identical frames: the 3DS LCD is
/// double-buffered): every [`FORMAT_CARDS`] card as a sprite (top row) and
/// as a textured 3D card (bottom row). With `photos`, the middle row shows
/// [`FORMAT_PHOTOS`] side by side (ETC1 blocks, RGBA4 banding): the 3DS
/// self-test dumps that frame as the `texture_formats.png` golden.
pub fn draw_texture_formats(renderer: &mut dyn Renderer, photos: bool) {
    let camera = formats_camera();
    for _ in 0..3 {
        renderer.begin_frame(Color::rgb(FORMATS_BG[0], FORMATS_BG[1], FORMATS_BG[2]));
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        for (i, (sprite, _)) in FORMAT_CARDS.iter().enumerate() {
            let s = FORMAT_CARD_SCALE;
            let transform = Transform3 {
                position: format_card_at(i),
                scale: Vec3::new(s, s, s),
                ..Transform3::default()
            };
            let params = ModelParams {
                shading: Shading::Unlit,
                texture: Some(*sprite),
                filter: TexFilter::Point,
                ..ModelParams::DEFAULT
            };
            renderer.draw_model(CARD, &transform, Material::VertexLit, &params);
        }
        for (i, (sprite, _)) in FORMAT_CARDS.iter().enumerate() {
            renderer.draw_sprite(*sprite, format_sprite_at(i), &SpriteParams::DEFAULT);
        }
        if photos {
            for (i, sprite) in FORMAT_PHOTOS.iter().enumerate() {
                let pos = Vec2::new(56.0 + 100.0 * i as f32, 56.0);
                renderer.draw_sprite(*sprite, pos, &SpriteParams::DEFAULT);
            }
        }
        renderer.end_frame();
    }
}

/// Texture formats: each [`FORMAT_CARDS`] card as a 2D sprite on a purple
/// background (top row: every quadrant, a transparent one must show the
/// background) and as a textured 3D card (bottom row: the opaque quadrants
/// only, since the 3D paths treat texture alpha differently). Catches a
/// format the shim samples wrong: an ignored ETC1A4 alpha, A8 drawn black,
/// L8 read as alpha, swapped channels. `Ok(false)` where `read_pixel` is
/// unsupported.
pub fn texture_formats(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = formats_camera();
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    draw_texture_formats(renderer, false);
    let near = |c: Color, want: [u8; 3]| {
        let d = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 40;
        d(c.r, want[0]) && d(c.g, want[1]) && d(c.b, want[2])
    };
    // Quadrant centers: (u, v) in the card.
    const CENTERS: [(f32, f32); 4] = [(0.25, 0.25), (0.75, 0.25), (0.25, 0.75), (0.75, 0.75)];
    for (i, (_, quadrants)) in FORMAT_CARDS.iter().enumerate() {
        for (q, want) in quadrants.iter().enumerate() {
            let (u, v) = CENTERS[q];
            let p = format_sprite_at(i);
            let Some(c) = renderer.read_pixel((p.x + 32.0 * u) as u32, (p.y + 32.0 * v) as u32)
            else {
                return Ok(false);
            };
            if !near(c, want.unwrap_or(FORMATS_BG)) {
                log_probe(platform, i, "2D", q, c, want.unwrap_or(FORMATS_BG));
                return Err("texture formats: a 2D sprite sampled its format wrong");
            }
            let Some(want) = want else { continue };
            // Front face at z = -0.05; planar UVs u = x + 0.5, v = 0.5 - y.
            let local = Vec3::new(u - 0.5, 0.5 - v, -0.05);
            let world = Vec3::new(
                format_card_at(i).x + local.x * FORMAT_CARD_SCALE,
                format_card_at(i).y + local.y * FORMAT_CARD_SCALE,
                local.z * FORMAT_CARD_SCALE,
            );
            let p = view.transform_point(world);
            let x = screen.x * 0.5 + p.x * focal / p.z;
            let y = screen.y * 0.5 - p.y * focal / p.z;
            let Some(c) = renderer.read_pixel(x as u32, y as u32) else {
                return Ok(false);
            };
            if !near(c, *want) {
                log_probe(platform, i, "3D", q, c, *want);
                return Err("texture formats: a 3D texture sampled its format wrong");
            }
        }
    }
    Ok(true)
}

/// Textured 3D: draw the quadrant card face-on and read back one pixel per
/// quadrant. Catches unbound textures, flipped/rotated UVs and broken
/// texture paths per backend. Skipped (Ok) where `read_pixel` is
/// unsupported (returns `Ok(false)`; `Ok(true)` = probed). Uses `caps()`
/// for the screen size (console profiles).
pub fn textured_3d(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let params = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(CARD, &Transform3::default(), Material::VertexLit, &params);
        renderer.end_frame();
    }
    // Project probe points exactly like the T&L does.
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    // Front face at z = -0.05; planar UVs u = x + 0.5, v = 0.5 - y.
    let probes = [
        (Vec3::new(-0.25, 0.25, -0.05), [255, 0, 0]), // u,v = .25,.25: TL red
        (Vec3::new(0.25, 0.25, -0.05), [0, 255, 0]),  // TR green
        (Vec3::new(-0.25, -0.25, -0.05), [0, 0, 255]), // BL blue
        (Vec3::new(0.25, -0.25, -0.05), [255, 255, 255]), // BR white
    ];
    for (world, want) in probes {
        let v = view.transform_point(world);
        let x = screen.x * 0.5 + v.x * focal / v.z;
        let y = screen.y * 0.5 - v.y * focal / v.z;
        let Some(got) = renderer.read_pixel(x as u32, y as u32) else {
            return Ok(false); // backend cannot read back
        };
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 48;
        if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
            return Err("textured 3D: wrong color in a texture quadrant");
        }
    }
    Ok(true)
}

/// `Renderer::fill_rect`: an opaque red rectangle with a half-transparent
/// white one over its left half, on black. Probes the opaque part (red),
/// the blend (pink: red + white at 50%) and the outside (black). `Ok(false)`
/// where `read_pixel` is unsupported.
pub fn fill_rect(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    let r = trino_core::Rect::new(w * 0.25, h * 0.25, w * 0.5, h * 0.5);
    let half = trino_core::Rect::new(w * 0.25, h * 0.25, w * 0.25, h * 0.5);
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.fill_rect(r, Color::rgb(255, 0, 0));
        renderer.fill_rect(half, Color::rgba(255, 255, 255, 128));
        renderer.end_frame();
    }
    let probes = [
        ((w * 0.6) as u32, (h * 0.5) as u32, [255, 0, 0]),
        ((w * 0.35) as u32, (h * 0.5) as u32, [255, 128, 128]),
        ((w * 0.1) as u32, (h * 0.1) as u32, [0, 0, 0]),
    ];
    for (x, y, want) in probes {
        let Some(got) = renderer.read_pixel(x, y) else {
            return Ok(false);
        };
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 40;
        if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
            return Err("fill_rect: wrong color (opaque, blended or outside)");
        }
    }
    Ok(true)
}

/// Two screens: `Renderer::on_screen` / `screen_size` /
/// `read_screen_pixel` and touch routing (`InputState::pointer_on`).
///
/// - A bottom-screen pointer is never reported to top-screen picking.
/// - With a second screen (3DS, the PC's 3DS profile): a strip on each
///   screen and the quadrant card in 3D on the bottom, then both screens
///   read back. The card's quadrants are probed where the *bottom's*
///   320x240 projection puts them (a top-screen projection leaking through
///   misses them), the top keeps its clear color where the card would land
///   there, and neither strip shows on the other screen (no missing or
///   shared framebuffer). Going back to the top after the bottom must be
///   refused.
/// - With one screen (N64, PC native): `on_screen(Bottom)` says no and
///   draws stay on the top: `Ok(false)`, also where read-back is missing.
pub fn dual_screen(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let mut touch = InputState::default();
    touch.pointer = Some(Vec2::new(40.0, 30.0));
    touch.pointer_screen = Screen::Bottom.index();
    if touch.pointer_on(Screen::Top).is_some() {
        return Err("dual screen: a bottom-screen touch reached top-screen picking");
    }
    if touch.pointer_on(Screen::Bottom) != Some(Vec2::new(40.0, 30.0)) {
        return Err("dual screen: a bottom-screen touch is missing on the bottom");
    }
    let top = renderer
        .screen_size(Screen::Top)
        .ok_or("dual screen: no top screen")?;
    let Some(bottom) = renderer.screen_size(Screen::Bottom) else {
        renderer.begin_frame(Color::BLACK);
        let refused = !renderer.on_screen(Screen::Bottom);
        let stayed = renderer.screen() == Screen::Top && renderer.on_screen(Screen::Top);
        renderer.end_frame();
        if !(refused && stayed) {
            return Err("dual screen: a single-screen target left the top screen");
        }
        return Ok(false);
    };
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.2),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let unlit = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let clear = [0, 0, 120];
    let (yellow, cyan) = (Color::rgb(255, 255, 0), Color::rgb(0, 255, 255));
    // Several identical frames: the 3DS LCDs are double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::rgb(clear[0], clear[1], clear[2]));
        if !renderer.on_screen(Screen::Top) {
            return Err("dual screen: on_screen(Top) refused at the frame's start");
        }
        renderer.fill_rect(Rect::new(0.0, 0.0, top.x, 16.0), yellow);
        if !renderer.on_screen(Screen::Bottom) {
            return Err("dual screen: on_screen(Bottom) refused with a second screen");
        }
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(CARD, &Transform3::default(), Material::VertexLit, &unlit);
        renderer.fill_rect(Rect::new(0.0, bottom.y - 16.0, bottom.x, 16.0), cyan);
        if renderer.on_screen(Screen::Top) || renderer.screen() != Screen::Bottom {
            return Err("dual screen: went back to the top after the bottom");
        }
        renderer.end_frame();
    }
    let close = |c: Color, want: [u8; 3], tol: i32| {
        (c.r as i32 - want[0] as i32).abs() <= tol
            && (c.g as i32 - want[1] as i32).abs() <= tol
            && (c.b as i32 - want[2] as i32).abs() <= tol
    };
    let Some(strip) = renderer.read_screen_pixel(Screen::Top, 8, 8) else {
        return Ok(false); // backend cannot read back
    };
    if !close(strip, [255, 255, 0], 40) {
        return Err("dual screen: the top strip is missing");
    }
    let mid = renderer.read_screen_pixel(Screen::Top, top.x as u32 / 2, top.y as u32 / 2);
    if !mid.is_some_and(|c| close(c, clear, 40)) {
        return Err("dual screen: bottom-screen 3D landed on the top screen");
    }
    let low = renderer.read_screen_pixel(Screen::Top, 8, top.y as u32 - 8);
    if !low.is_some_and(|c| close(c, clear, 40)) {
        return Err("dual screen: the bottom strip landed on the top screen");
    }
    let at = |r: &mut dyn Renderer, x: f32, y: f32| {
        r.read_screen_pixel(Screen::Bottom, x as u32, y as u32)
    };
    if !at(renderer, 8.0, bottom.y - 8.0).is_some_and(|c| close(c, [0, 255, 255], 40)) {
        return Err("dual screen: the bottom strip is missing (no bottom framebuffer?)");
    }
    if at(renderer, 8.0, 8.0).is_none_or(|c| close(c, [255, 255, 0], 80)) {
        return Err("dual screen: the top strip landed on the bottom screen");
    }
    // The card through the bottom's projection: near its corners and just
    // outside its left and right edges. A 400-wide projection shifts the
    // card 40 px right, enough to move both kinds of probe.
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * bottom.y * 0.5;
    let project = |world: Vec3| {
        let v = view.transform_point(world);
        (
            bottom.x * 0.5 + v.x * focal / v.z,
            bottom.y * 0.5 - v.y * focal / v.z,
        )
    };
    let corners = [
        (Vec3::new(-0.35, 0.35, -0.05), [255, 0, 0]),
        (Vec3::new(0.35, 0.35, -0.05), [0, 255, 0]),
        (Vec3::new(-0.35, -0.35, -0.05), [0, 0, 255]),
        (Vec3::new(0.35, -0.35, -0.05), [255, 255, 255]),
    ];
    for (world, want) in corners {
        let (x, y) = project(world);
        if !at(renderer, x, y).is_some_and(|c| close(c, want, 48)) {
            return Err("dual screen: wrong color in the bottom screen's 3D card");
        }
    }
    for side in [-0.7, 0.7] {
        let (x, y) = project(Vec3::new(side, 0.0, -0.05));
        if !at(renderer, x, y).is_some_and(|c| c.r < 60 && c.g < 60 && c.b < 60) {
            return Err("dual screen: the bottom screen's 3D card is off its projection");
        }
    }
    Ok(true)
}

/// Unit block (1x1x1, centered) used by [`occlusion`].
const BLOCK: ModelId = ModelId::from_path("models/c64_block_stone");

/// Depth occlusion: boxes standing on a large floor, seen from a
/// third-person camera — castle64's player and boars on the ground. Probes
/// each box's front face just above the floor: it must show the box, never
/// the floor. A backend that resolves depth per triangle instead of per
/// pixel paints big floor triangles over whatever stands on them (the
/// original 3DS path: citro2d's one averaged depth per triangle). The PC
/// and N64 z-buffers pass trivially; the check exists so no backend can
/// regress to per-triangle depth. `Ok(false)` where `read_pixel` is
/// unsupported.
pub fn occlusion(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 2.6, -6.0),
        target: Vec3::new(0.0, 0.4, 0.0),
        ..Camera3::default()
    };
    // Flat colors whatever the block's vertex colors: tint black for the
    // floor; the boxes' tint zeroes green and the emissive saturates red
    // and blue — pure magenta.
    let floor = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgba(0, 0, 0, 255),
        ..ModelParams::DEFAULT
    };
    let boxes = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgba(255, 0, 255, 255),
        emissive: Color::rgba(255, 0, 255, 255),
        ..ModelParams::DEFAULT
    };
    let spots = [
        Vec3::new(-2.2, 0.5, -1.0),
        Vec3::new(0.0, 0.5, 0.0),
        Vec3::new(2.0, 0.5, 2.5),
        Vec3::new(-1.0, 0.5, 5.0),
    ];
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::rgba(40, 60, 200, 255));
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(
            BLOCK,
            &Transform3 {
                position: Vec3::new(0.0, -0.5, 4.0),
                rotation: Vec3::ZERO,
                scale: Vec3::new(30.0, 1.0, 30.0),
            },
            Material::VertexLit,
            &floor,
        );
        for at in spots {
            let t = Transform3 {
                position: at,
                ..Transform3::default()
            };
            renderer.draw_model(BLOCK, &t, Material::VertexLit, &boxes);
        }
        renderer.end_frame();
    }
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let pixel = |renderer: &mut dyn Renderer, world: Vec3| {
        let v = view.transform_point(world);
        let x = screen.x * 0.5 + v.x * focal / v.z;
        let y = screen.y * 0.5 - v.y * focal / v.z;
        renderer.read_pixel(x as u32, y as u32)
    };
    // The floor itself rendered (in front of the first box).
    let Some(f) = pixel(renderer, Vec3::new(0.0, 0.0, -2.0)) else {
        return Ok(false); // backend cannot read back
    };
    if f.r > 60 || f.g > 60 || f.b > 60 {
        return Err("occlusion: the floor did not render");
    }
    for at in spots {
        // Front face (z - 0.5), low: 0.08 and 0.25 above the floor.
        for (dx, h) in [(0.0, 0.08), (-0.25, 0.25), (0.25, 0.25)] {
            let p = Vec3::new(at.x + dx, h, at.z - 0.5);
            let Some(c) = pixel(renderer, p) else {
                return Ok(false);
            };
            if !(c.r > 150 && c.g < 100 && c.b > 150) {
                return Err("occlusion: the floor painted over a box standing on it");
            }
        }
    }
    Ok(true)
}

/// Bitmap text: the font's glyph table loaded (metrics) and drawing emits
/// one sprite per visible glyph (the atlas + sprite regions work).
pub fn text(renderer: &mut dyn Renderer) -> Result<(), &'static str> {
    // trino5x7: 'x' and '0' are 5 px wide + 1 px spacing; cells are 10 tall.
    if renderer.measure_text(FONT, "x00", 1.0) != Vec2::new(18.0, 10.0) {
        return Err("font metrics (glyph table missing or wrong)");
    }
    renderer.begin_frame(Color::BLACK);
    renderer.draw_text(FONT, Vec2::new(4.0, 4.0), "TRINO 64", &TextParams::DEFAULT);
    renderer.end_frame();
    // 7 glyphs: the space advances without drawing.
    if renderer.stats().sprites != 7 {
        return Err("draw_text glyph count");
    }
    Ok(())
}

/// Data blobs arrive byte-for-byte.
pub fn blob(platform: &mut dyn Platform) -> Result<(), &'static str> {
    match platform.blob(BLOB) {
        Some(bytes) if bytes == BLOB_CONTENT => Ok(()),
        Some(_) => Err("blob content mismatch"),
        None => Err("blob missing"),
    }
}

/// The localized font (`[fonts.i18n]`: `chars = "auto"`, one atlas per
/// language).
pub const I18N_FONT: FontId = FontId::from_path("fonts/i18n");

/// Localization (issue #59) end to end: every baked language loads from
/// `i18n/languages`, the system language picks one, and in each of en-US,
/// pt-BR and es-ES a key, its fallback, a plural and a price come out in
/// the language's words and number format, drawn with the language's own
/// glyph-subset atlas. Pixels: the price's last glyph columns are lit and
/// the columns just past its measured width are dark, so the drawn text is
/// the language's (the three prices differ in width). `Ok(false)` where
/// `read_pixel` is unsupported (the logic still ran).
#[inline(never)]
pub fn i18n(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::i18n::{Arg, LangId, Localizer, StrKey};
    const HELLO: StrKey = StrKey::new("selftest.hello");
    const PRICE: StrKey = StrKey::new("selftest.price");
    const COINS: StrKey = StrKey::new("selftest.coins");
    const ONLY_EN: StrKey = StrKey::new("selftest.only_en");
    let mut loc: Localizer<'static, 4> = Localizer::new();
    if loc.load(platform) < 3 {
        return Err("i18n: i18n/languages or a language's table is missing");
    }
    if loc.tag() != "en-US" {
        return Err("i18n: the default language is not loaded first");
    }
    // Whatever the system says, the pick is a loaded language.
    let system = platform.system_language();
    let picked = loc.pick(None, system);
    if loc.table(picked).is_none() {
        return Err("i18n: the system language's pick is not a loaded language");
    }
    let cases: [(&str, &str, &str, &str, &str, &str); 3] = [
        (
            "en-US",
            "Hello",
            "Price: $1,200.00",
            "1 coin",
            "0 coins",
            "fonts/i18n",
        ),
        (
            "pt-BR",
            "Olá",
            "Preço: R$ 1.200,00",
            "1 moeda",
            "0 moeda",
            "fonts/i18n@pt-BR",
        ),
        (
            "es-ES",
            "Hola",
            "Precio: 1200,00 €",
            "1 moneda",
            "0 monedas",
            "fonts/i18n@es-ES",
        ),
    ];
    let mut probed = true;
    let mut buf: TextBuf<48> = TextBuf::new();
    for (tag, hello, price, one, zero, atlas) in cases {
        if !loc.set_language(LangId::new(tag)) {
            return Err("i18n: a language did not load");
        }
        if loc.tr(HELLO) != hello {
            return Err("i18n: a key is not in the language's words");
        }
        if loc.tr(ONLY_EN) != "Fallback" {
            return Err("i18n: a missing key did not fall back to en-US");
        }
        if loc.tr_fmt(COINS, &[Arg::int("n", 1)], &mut buf) != one
            || loc.tr_fmt(COINS, &[Arg::int("n", 0)], &mut buf) != zero
        {
            return Err("i18n: a plural picked the wrong form");
        }
        let font = loc.font(I18N_FONT);
        if font != FontId::from_path(atlas) {
            return Err("i18n: the language does not draw with its own atlas");
        }
        let text = loc.tr_fmt(PRICE, &[Arg::money("amount", 120_000)], &mut buf);
        if text != price {
            return Err("i18n: a price is not in the language's number format");
        }
        let size = renderer.measure_text(font, text, 1.0);
        if size.x < 20.0 || size.y < 8.0 {
            return Err("i18n: the language's atlas is not loaded");
        }
        if renderer.measure_text(I18N_FONT, "Hello", 1.0).x < 10.0 {
            return Err("i18n: the default atlas is not loaded");
        }
        // Several identical frames: the 3DS LCDs are double-buffered.
        let at = Vec2::new(8.0, 8.0);
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.draw_text(font, at, text, &TextParams::DEFAULT);
            renderer.end_frame();
        }
        let lit = |r: &mut dyn Renderer, x0: f32, x1: f32| -> Option<bool> {
            let mut any = false;
            let mut x = x0;
            while x < x1 {
                for y in (at.y as u32 + 1..(at.y + size.y) as u32).step_by(2) {
                    let c = r.read_pixel(x as u32, y)?;
                    any |= c.r as u16 + c.g as u16 + c.b as u16 > 240;
                }
                x += 1.0;
            }
            Some(any)
        };
        let end = at.x + size.x;
        let last = lit(renderer, end - 4.0, end);
        match (last, lit(renderer, end + 3.0, end + 14.0)) {
            (Some(true), Some(false)) => {}
            (Some(_), Some(_)) => {
                return Err("i18n: the drawn price does not end where its language's ends");
            }
            _ => probed = false,
        }
    }
    // Back to the language the system picked.
    loc.set_language(picked);
    Ok(probed)
}

/// `ui/selftest.ui.ron`, baked as a blob on every target (issue #58).
pub const UI_SCREEN: BlobId = BlobId::from_path("ui/selftest");

/// The UI system (issue #58) end to end: the baked `ui/selftest` screen
/// parses, lays out inside the target's safe area, draws its focused
/// button in the theme's focus color, moves the focus with the D-pad,
/// clicks with A, and a touch on the bottom screen (the main one where
/// there is one screen) taps the `touch` button. `Ok(false)` where
/// `read_pixel` is unsupported (the logic still ran).
#[inline(never)]
pub fn ui_layout(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::ui::{Theme, Ui, UiEnv, UiScreen};
    let bytes = platform
        .blob(UI_SCREEN)
        .ok_or("ui layout: blob ui/selftest missing")?;
    let screen = UiScreen::parse(bytes).map_err(|_| "ui layout: ui/selftest does not parse")?;
    let mut ui: Ui<16> = Ui::new();
    let env = UiEnv::of(renderer);
    let idle = InputState::default();
    let frame = |r: &mut dyn Renderer, ui: &mut Ui<16>| {
        // Several identical frames: the 3DS LCDs are double-buffered.
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            ui.draw(r, &screen, &env);
            r.end_frame();
        }
    };
    frame(renderer, &mut ui);
    let _ = ui.update(&screen, &idle, &idle, 0.0);
    if ui.focused(&screen).map(|f| f.0) != Some("one") {
        return Err("ui layout: the first button is not focused");
    }
    if ui.unreachable(&screen).is_some() {
        return Err("ui layout: the pad cannot reach every button");
    }
    // The stack sits inside the safe area, centered.
    let (_, one) = ui.rect_of(&screen, "one").ok_or("ui layout: no `one`")?;
    let top = renderer
        .screen_size(Screen::Top)
        .ok_or("ui layout: no screen")?;
    let safe = renderer.safe_area(Screen::Top);
    let mid = one.pos.x + one.size.x * 0.5;
    if (mid - top.x * 0.5).abs() > 1.0 || one.pos.y < safe.top as f32 {
        return Err("ui layout: the stack is not centered in the safe area");
    }
    // D-pad down: `two` takes the focus; A clicks it.
    let mut down = InputState::default();
    down.set(trino_core::Button::DpadDown, true);
    let _ = ui.update(&screen, &down, &idle, 1.0 / 60.0);
    let _ = ui.update(&screen, &idle, &down, 1.0 / 60.0);
    if ui.focused(&screen).map(|f| f.0) != Some("two") {
        return Err("ui layout: D-pad down did not move the focus to `two`");
    }
    let mut a = InputState::default();
    a.set(trino_core::Button::A, true);
    if ui.update(&screen, &a, &idle, 1.0 / 60.0).clicked() != Some("two") {
        return Err("ui layout: A did not click the focused button");
    }
    let _ = ui.update(&screen, &idle, &a, 1.0 / 60.0);
    frame(renderer, &mut ui);
    let (_, two) = ui.rect_of(&screen, "two").ok_or("ui layout: no `two`")?;
    let theme = Theme::DEFAULT;
    // Left of the centered label, inside the focus ring.
    let at = |r: Rect| {
        (
            (r.pos.x + r.size.x * 0.1) as u32,
            (r.pos.y + r.size.y * 0.5) as u32,
        )
    };
    let close = |c: Color, want: Color| {
        (c.r as i32 - want.r as i32).abs() <= 40
            && (c.g as i32 - want.g as i32).abs() <= 40
            && (c.b as i32 - want.b as i32).abs() <= 40
    };
    let (x, y) = at(two);
    let Some(focused) = renderer.read_screen_pixel(Screen::Top, x, y) else {
        return ui_touch(&screen, &mut ui).map(|_| false);
    };
    if !close(focused, theme.focus) {
        return Err("ui layout: the focused button is not in the focus color");
    }
    let (x, y) = at(one);
    if !renderer
        .read_screen_pixel(Screen::Top, x, y)
        .is_some_and(|c| close(c, theme.button))
    {
        return Err("ui layout: an unfocused button is not in the button color");
    }
    ui_touch(&screen, &mut ui)?;
    // The tap focused `touch`: probe it where it is drawn.
    frame(renderer, &mut ui);
    let (on, touch) = ui
        .rect_of(&screen, "touch")
        .ok_or("ui layout: no `touch`")?;
    let (x, y) = at(touch);
    if !renderer
        .read_screen_pixel(on, x, y)
        .is_some_and(|c| close(c, theme.focus))
    {
        return Err("ui layout: the tapped button is not focused on its screen");
    }
    Ok(true)
}

/// A tap (press, then the finger lifts with no position, as on the 3DS)
/// on the `touch` button's screen clicks it.
fn ui_touch(
    screen: &trino_core::ui::UiScreen<'_>,
    ui: &mut trino_core::ui::Ui<16>,
) -> Result<(), &'static str> {
    let (on, touch) = ui.rect_of(screen, "touch").ok_or("ui layout: no `touch`")?;
    let mut press = InputState::default();
    press.pointer = Some(Vec2::new(
        touch.pos.x + touch.size.x * 0.5,
        touch.pos.y + touch.size.y * 0.5,
    ));
    press.pointer_screen = on.index();
    press.set_pointer_button(trino_core::PointerButton::Primary, true);
    let _ = ui.update(screen, &press, &InputState::default(), 1.0 / 60.0);
    let lift = InputState::default();
    if ui.update(screen, &lift, &press, 1.0 / 60.0).clicked() != Some("touch") {
        return Err("ui layout: a tap did not click the touch button");
    }
    if ui.focused(screen).map(|f| f.0) != Some("touch") {
        return Err("ui layout: a tap did not focus the touch button");
    }
    Ok(())
}

/// Save data round-trips through the real medium (EEPROM / SD / file).
pub fn save(platform: &mut dyn Platform) -> Result<(), &'static str> {
    if platform.save_capacity() < 16 {
        return Err("no save medium");
    }
    let data = b"trino-save-v1";
    platform.save_write(data).map_err(|_| "save write")?;
    let mut back = [0u8; 32];
    let n = platform.save_read(&mut back).map_err(|_| "save read")?;
    if n != data.len() || &back[..n] != data {
        return Err("save read-back mismatch");
    }
    Ok(())
}

/// The quadrant texture as a sprite (billboards sample regions of it).
pub const QUADRANTS: SpriteId = SpriteId::from_path("sprites/selftest_quadrants");

/// Billboards + particles: two camera-facing quads showing single texture
/// quadrants (a sprite region, clamped — no bleeding from the neighbour),
/// probed by pixel; then a particle burst must emit one billboard per
/// particle. `Ok(false)` where `read_pixel` is unsupported.
pub fn billboards(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -4.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let quad = |region: Rect| BillboardParams {
        size: Vec2::new(1.2, 1.2),
        region: Some(region),
        ..BillboardParams::DEFAULT
    };
    let left = Vec3::new(-1.0, 0.0, 0.0);
    let right = Vec3::new(1.0, 0.0, 0.0);
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        // Top-left quadrant (red) on the left, bottom-right (white) right.
        renderer.draw_billboard(QUADRANTS, left, &quad(Rect::new(0.0, 0.0, 8.0, 8.0)));
        renderer.draw_billboard(QUADRANTS, right, &quad(Rect::new(8.0, 8.0, 8.0, 8.0)));
        renderer.end_frame();
    }
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let mut probed = true;
    for (world, want) in [(left, [255u8, 0, 0]), (right, [255, 255, 255])] {
        let v = view.transform_point(world);
        let x = screen.x * 0.5 + v.x * focal / v.z;
        let y = screen.y * 0.5 - v.y * focal / v.z;
        let Some(got) = renderer.read_pixel(x as u32, y as u32) else {
            probed = false;
            break;
        };
        let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 48;
        if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
            return Err("billboard: wrong color (region or clamp broken)");
        }
    }
    // Particles: a burst of 12 draws 12 billboards (all in front of the
    // camera: a slow upward puff at the origin).
    let mut puff: ParticleSystem<16> = ParticleSystem::new(
        Emitter {
            sprite: QUADRANTS,
            rate: 0.0,
            life: (1.0, 1.0),
            speed: (0.1, 0.2),
            size: (0.3, 0.3),
            ..Emitter::DEFAULT
        },
        5,
    );
    puff.burst(12);
    puff.update(0.1);
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&camera);
    puff.draw(renderer);
    renderer.end_frame();
    if renderer.stats().sprites != 12 {
        return Err("particles: expected one billboard per particle");
    }
    Ok(probed)
}

/// The looping water ambience (`[sounds.water]`, `loop = true`).
pub const WATER: trino_core::SoundId = trino_core::SoundId::from_path("sounds/water");
/// A stinger track (`[music.fanfare]`).
pub const FANFARE: trino_core::MusicId = trino_core::MusicId::from_path("music/fanfare");

/// The backend's input with nothing pressed (the emulator harnesses press
/// nothing): no button, key, right stick or trigger. On consoles this runs
/// the shims' reads (the N64's C-button bits, the 3DS's ZL/ZR and C-stick),
/// so a bit packed into the wrong place fails here.
pub fn idle_input_checks(input: &mut dyn trino_core::Input) -> Result<(), &'static str> {
    for _ in 0..3 {
        let s = input.poll();
        if s.any_button() {
            return Err("input: a button reads held with nothing pressed");
        }
        if !s.keys.is_empty() {
            return Err("input: a key reads held with nothing pressed");
        }
        if s.right_stick != Vec2::ZERO {
            return Err("input: the right stick is off center with nothing pressed");
        }
        if s.triggers != Vec2::ZERO {
            return Err("input: a trigger reads pulled with nothing pressed");
        }
    }
    Ok(())
}

/// Audio v2 through the real backend: a steerable looping voice (start,
/// steer, stop) and a stinger over the music.
pub fn audio_checks(audio: &mut dyn trino_core::Audio) -> Result<(), &'static str> {
    use trino_core::SoundParams;
    let left = SoundParams {
        volume: 0.5,
        pan: -1.0,
        ..SoundParams::DEFAULT
    };
    let voice = audio
        .play_voice(WATER, &left, true)
        .ok_or("no voice for a looping sound")?;
    if !audio.voice_playing(voice) {
        return Err("voice not playing after play_voice");
    }
    audio.set_voice(voice, &SoundParams::DEFAULT);
    audio.stop_voice(voice);
    if audio.voice_playing(voice) {
        return Err("voice still playing after stop_voice");
    }
    audio.play_stinger(FANFARE);
    if !audio.stinger_playing() {
        return Err("stinger not playing");
    }
    audio.play_sound_with(
        WATER,
        &SoundParams {
            volume: 0.2,
            pan: 0.5,
            ..SoundParams::DEFAULT
        },
    );
    Ok(())
}

/// The `audio codecs` step's sounds: the same beep baked as PCM and as the
/// platform's default codec (VADPCM, DSP-ADPCM, Vorbis), with what each
/// proves (issue #53).
pub const AUDIO_CODEC_SOUNDS: [(trino_core::SoundId, &str); 2] = [
    (
        trino_core::SoundId::from_path("sounds/selftest_pcm"),
        "audio codecs: the PCM sound",
    ),
    (
        trino_core::SoundId::from_path("sounds/selftest_codec"),
        "audio codecs: the platform-codec sound",
    ),
];
/// A short track baked as Opus on the N64 (the platform default elsewhere),
/// played through to its end.
pub const AUDIO_OPUS: trino_core::MusicId = trino_core::MusicId::from_path("music/selftest_opus");
/// The streamed track (`[music.theme]`, music streams by default).
pub const AUDIO_STREAM: trino_core::MusicId = trino_core::MusicId::from_path("music/theme");
/// Frames the stream must play for, and the samples it must deliver in
/// them (half a second at the lowest rate a backend uses).
pub const AUDIO_STREAM_FRAMES: u32 = 60;
pub const AUDIO_STREAM_MIN_SAMPLES: u32 = 4_000;
/// Frames a ramp gets to settle (attack and release are 50 ms).
const AUDIO_DUCK_FRAMES: u32 = 15;
/// Frames a one-shot gets to finish before decoding counts as stalled.
const AUDIO_SOUND_TIMEOUT: u32 = 120;

#[derive(Clone, Copy, PartialEq, Debug)]
enum CodecPhase {
    Start,
    Sound(usize),
    Opus,
    Stream,
    Stopped,
    Duck,
    Release,
    Done,
}

/// Step `audio codecs` (issue #53) as a state machine, one call per frame
/// (the Web's animation frames drive it; [`audio_codecs`] loops it on PC
/// and consoles): the bus API reads back, each codec plays and finishes,
/// an Opus/default-codec track plays to its end, the streamed track
/// delivers samples with no underruns and releases its buffers, and a duck
/// rule ramps the music down and back. Leaves the buses as it found them.
pub struct AudioCodecs {
    phase: CodecPhase,
    frames: u32,
    voice: Option<trino_core::VoiceId>,
    volumes: [f32; trino_core::Bus::COUNT],
    base: trino_core::AudioStats,
    /// The stats after the stream played (what the runners log).
    pub streamed: trino_core::AudioStats,
}

impl Default for AudioCodecs {
    fn default() -> Self {
        Self::new()
    }
}

impl AudioCodecs {
    pub const DUCK: trino_core::DuckRule =
        trino_core::DuckRule::new(trino_core::Bus::Sfx, trino_core::Bus::Music, 0.4, 50, 50);

    pub fn new() -> Self {
        AudioCodecs {
            phase: CodecPhase::Start,
            frames: 0,
            voice: None,
            volumes: [1.0; trino_core::Bus::COUNT],
            base: trino_core::AudioStats::default(),
            streamed: trino_core::AudioStats::default(),
        }
    }

    /// Advance by one frame (call it, then let a frame of audio play). True
    /// once the step passed.
    pub fn step(&mut self, audio: &mut dyn trino_core::Audio) -> Result<bool, &'static str> {
        use trino_core::{Bus, SoundParams};
        self.frames += 1;
        match self.phase {
            CodecPhase::Start => {
                for b in Bus::ALL {
                    self.volumes[b.index()] = audio.bus_volume(b);
                }
                audio.set_bus_volume(Bus::Sfx, 0.5);
                if (audio.bus_volume(Bus::Sfx) - 0.5).abs() > 0.01 {
                    return Err("audio codecs: a bus volume doesn't read back");
                }
                audio.set_master_volume(0.75);
                audio.set_music_volume(0.6);
                if (audio.bus_volume(Bus::Master) - 0.75).abs() > 0.01
                    || (audio.bus_volume(Bus::Music) - 0.6).abs() > 0.01
                {
                    return Err("audio codecs: set_master/music_volume don't map to the buses");
                }
                audio.set_bus_muted(Bus::Ui, true);
                if !audio.bus_muted(Bus::Ui) || audio.bus_muted(Bus::Sfx) {
                    return Err("audio codecs: a bus mute doesn't read back");
                }
                audio.set_bus_muted(Bus::Ui, false);
                self.restore(audio);
                if audio.audio_stats().resident_bytes == 0 {
                    return Err("audio codecs: no sound memory reported");
                }
                self.start_sound(audio, 0)?;
            }
            CodecPhase::Sound(i) => {
                let v = self.voice.ok_or("audio codecs: lost the voice")?;
                // `start_sound` saw it playing; how soon it ends depends on
                // the frame time (a slow frame outlasts a 0.15 s beep).
                if !audio.voice_playing(v) {
                    if i + 1 < AUDIO_CODEC_SOUNDS.len() {
                        self.start_sound(audio, i + 1)?;
                    } else {
                        audio.play_music(AUDIO_OPUS, false);
                        self.enter(CodecPhase::Opus);
                    }
                } else if self.frames > AUDIO_SOUND_TIMEOUT {
                    return Err("audio codecs: a sound never finished (decoding stalled)");
                }
            }
            CodecPhase::Opus => {
                let s = audio.audio_stats();
                if self.frames == 2 && s.voices == 0 {
                    return Err("audio codecs: the Opus/codec track isn't playing");
                }
                if self.frames > 2 && s.voices == 0 {
                    audio.play_music(AUDIO_STREAM, false);
                    self.base = audio.audio_stats();
                    self.enter(CodecPhase::Stream);
                } else if self.frames > AUDIO_SOUND_TIMEOUT {
                    return Err("audio codecs: the Opus/codec track never finished");
                }
            }
            CodecPhase::Stream => {
                if self.frames >= AUDIO_STREAM_FRAMES {
                    let s = audio.audio_stats();
                    if s.streams == 0 || s.stream_bytes == 0 {
                        return Err("audio codecs: the music isn't streaming");
                    }
                    let got = s.streamed_frames.wrapping_sub(self.base.streamed_frames);
                    if got < AUDIO_STREAM_MIN_SAMPLES {
                        return Err("audio codecs: the stream delivered too few samples");
                    }
                    if s.underruns != self.base.underruns {
                        return Err("audio codecs: the stream underran");
                    }
                    self.streamed = s;
                    audio.stop_music();
                    self.enter(CodecPhase::Stopped);
                }
            }
            CodecPhase::Stopped => {
                if self.frames >= 2 {
                    if audio.audio_stats().streams != 0 {
                        return Err("audio codecs: a stopped stream kept its buffers");
                    }
                    audio.set_duck(Self::DUCK);
                    audio.play_music(AUDIO_STREAM, true);
                    let v = audio
                        .play_voice(WATER, &SoundParams::DEFAULT, true)
                        .ok_or("audio codecs: no voice for the ducking sound")?;
                    self.voice = Some(v);
                    self.enter(CodecPhase::Duck);
                }
            }
            CodecPhase::Duck => {
                if self.frames >= AUDIO_DUCK_FRAMES {
                    if audio.bus_duck(Bus::Music) > 0.45 {
                        return Err("audio codecs: the music didn't duck under the sfx");
                    }
                    if let Some(v) = self.voice.take() {
                        audio.stop_voice(v);
                    }
                    self.enter(CodecPhase::Release);
                }
            }
            CodecPhase::Release => {
                if self.frames >= AUDIO_DUCK_FRAMES {
                    if audio.bus_duck(Bus::Music) < 0.95 {
                        return Err("audio codecs: the music didn't come back after the duck");
                    }
                    let mut off = Self::DUCK;
                    off.level = 1.0;
                    audio.set_duck(off);
                    audio.stop_music();
                    self.restore(audio);
                    self.enter(CodecPhase::Done);
                }
            }
            CodecPhase::Done => {}
        }
        Ok(self.phase == CodecPhase::Done)
    }

    fn enter(&mut self, phase: CodecPhase) {
        self.phase = phase;
        self.frames = 0;
    }

    fn start_sound(
        &mut self,
        audio: &mut dyn trino_core::Audio,
        i: usize,
    ) -> Result<(), &'static str> {
        let (id, what) = AUDIO_CODEC_SOUNDS[i];
        let v = audio
            .play_voice(id, &trino_core::SoundParams::DEFAULT, false)
            .ok_or(what)?;
        if !audio.voice_playing(v) {
            return Err(what);
        }
        self.voice = Some(v);
        self.enter(CodecPhase::Sound(i));
        Ok(())
    }

    fn restore(&self, audio: &mut dyn trino_core::Audio) {
        for b in trino_core::Bus::ALL {
            audio.set_bus_volume(b, self.volumes[b.index()]);
        }
    }
}

/// [`AudioCodecs`] to the end: `frame` lets one frame of audio play (the
/// consoles poll their audio and wait a vblank; the PC renders 1/60 s
/// offline). The stats after the stream played, for the runners' logs.
pub fn audio_codecs<A: trino_core::Audio>(
    audio: &mut A,
    mut frame: impl FnMut(&mut A),
) -> Result<trino_core::AudioStats, &'static str> {
    let mut step = AudioCodecs::new();
    while !step.step(audio)? {
        frame(audio);
    }
    Ok(step.streamed)
}

/// A smooth-skinned strip (`[models.selftest_strip]`, `skinning = "smooth"`)
/// draws through the two-joint path: its triangle reaches the rasterizer.
pub fn smooth_skin(renderer: &mut dyn Renderer) -> Result<(), &'static str> {
    const STRIP: ModelId = ModelId::from_path("models/selftest_strip");
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.1, 0.5, -2.0),
        target: Vec3::new(0.1, 0.5, 0.0),
        fov_y: 1.0,
    });
    renderer.draw_model(
        STRIP,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams {
            double_sided: true,
            ..ModelParams::DEFAULT
        },
    );
    renderer.end_frame();
    if renderer.stats().tris == 0 {
        return Err("smooth-skinned model drew nothing (SKW2 path)");
    }
    Ok(())
}

/// A white card in color slots 0, 1 and 4 (`[models.selftest_slots]`,
/// glTF `_SLOT`).
pub const SLOTS_CARD: ModelId = ModelId::from_path("models/selftest_slots");

/// Color slots and a per-draw palette: the slotted card drawn face-on with
/// a palette shows white (slot 0), the first entry (slot 1) and the fourth
/// (slot 4); drawn again without one it is white everywhere. On the N64 the
/// palette draw must stay off the RSP overlay (which draws baked colors),
/// and the plain one may take it. `Ok(false)` where `read_pixel` is
/// unsupported.
pub fn palette(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let (red, blue) = ([230, 40, 40], [40, 60, 230]);
    let rgb = |c: [u8; 3]| Color::rgb(c[0], c[1], c[2]);
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    for (palette, want) in [
        (
            Some([rgb(red), Color::WHITE, Color::WHITE, rgb(blue)]),
            [[255, 255, 255], red, blue],
        ),
        (None, [[255, 255, 255]; 3]),
    ] {
        let params = ModelParams {
            shading: Shading::Unlit,
            palette,
            ..ModelParams::DEFAULT
        };
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.draw_model(
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &params,
            );
            renderer.end_frame();
        }
        // Column centers: slot 0 at x = -0.4, slot 1 at 0, slot 4 at 0.4.
        for (x, want) in [-0.4f32, 0.0, 0.4].into_iter().zip(want) {
            let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
            let px = screen.x * 0.5 + v.x * focal / v.z;
            let py = screen.y * 0.5 - v.y * focal / v.z;
            let Some(got) = renderer.read_pixel(px as u32, py as u32) else {
                return Ok(false);
            };
            let close = |a: u8, b: u8| (a as i32 - b as i32).abs() <= 48;
            if !(close(got.r, want[0]) && close(got.g, want[1]) && close(got.b, want[2])) {
                return Err(if palette.is_some() {
                    "palette: a color slot shows the wrong color"
                } else {
                    "palette: without a palette the card is not its baked white"
                });
            }
        }
    }
    Ok(true)
}

/// Copies the `instancing` step draws.
pub const INSTANCES: usize = 6;

/// The `instancing` step's scene: [`INSTANCES`] small slotted cards in a
/// 3 × 2 grid, each its own tint, two with a palette from the table, and
/// the call's own palette for the rest.
pub fn instancing_scene() -> ([Instance; INSTANCES], [Palette; 2], ModelParams) {
    let rgb = Color::rgb;
    let tints = [
        rgb(255, 80, 80),
        rgb(80, 255, 80),
        rgb(80, 80, 255),
        Color::WHITE,
        rgb(255, 255, 80),
        rgb(80, 255, 255),
    ];
    let palette_index = [0, 1, 0, 2, 7, 1];
    let instances = core::array::from_fn(|i| {
        let (col, row) = ((i % 3) as f32, (i / 3) as f32);
        Instance {
            matrix: Transform3 {
                position: Vec3::new(-0.6 + col * 0.6, 0.35 - row * 0.7, 0.0),
                rotation: Vec3::ZERO,
                scale: Vec3::new(0.45, 0.45, 0.45),
            }
            .matrix(),
            tint: tints[i],
            palette_index: palette_index[i],
        }
    });
    let palettes = [
        [
            rgb(230, 40, 40),
            Color::WHITE,
            Color::WHITE,
            rgb(40, 60, 230),
        ],
        [rgb(40, 200, 40); 4],
    ];
    let params = ModelParams {
        shading: Shading::Unlit,
        tint: rgb(255, 255, 200),
        palette: Some([rgb(200, 120, 40); 4]),
        ..ModelParams::DEFAULT
    };
    (instances, palettes, params)
}

/// Instancing ([`Renderer::draw_model_instances_with`], issue #51): the
/// same copies drawn as separate `draw_model_matrix` calls (their tint and
/// palette worked out here, by the rules [`Instance`] documents) and then
/// in one call must give the same pixels — a grid of probes plus each
/// card's center — and the same counters (the `instanced` count aside:
/// zero for the trait's default loop, one per copy where a backend counts
/// them). Then the first card must show its own tint. `Ok(false)` where
/// `read_pixel` is unsupported (the counters are still compared).
#[inline(never)]
pub fn instancing(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let (instances, palettes, params) = instancing_scene();
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let project = |p: Vec3| {
        let v = view.transform_point(p);
        (
            (screen.x * 0.5 + v.x * focal / v.z) as u32,
            (screen.y * 0.5 - v.y * focal / v.z) as u32,
        )
    };
    // 8 × 6 probes over the screen, then each card's center.
    let mut probes = [(0u32, 0u32); 48 + INSTANCES];
    for (k, p) in probes.iter_mut().enumerate().take(48) {
        let (gx, gy) = ((k % 8) as u32, (k / 8) as u32);
        *p = (
            (gx * 2 + 1) * screen.x as u32 / 16,
            (gy * 2 + 1) * screen.y as u32 / 12,
        );
    }
    for (p, i) in probes[48..].iter_mut().zip(&instances) {
        *p = project(i.matrix.transform_point(Vec3::ZERO));
    }
    let mul = |a: u8, b: u8| ((a as u16 * b as u16 + 127) / 255) as u8;
    let mut seen = [[Color::BLACK; 48 + INSTANCES]; 2];
    let mut stats = [trino_core::FrameStats::default(); 2];
    let mut read = true;
    for (pass, (seen, stats)) in seen.iter_mut().zip(&mut stats).enumerate() {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            if pass == 0 {
                for i in &instances {
                    let (a, b) = (params.tint, i.tint);
                    let palette = match i.palette_index {
                        0 => params.palette,
                        k => palettes.get(k as usize - 1).copied().or(params.palette),
                    };
                    let own = ModelParams {
                        tint: Color::rgba(
                            mul(a.r, b.r),
                            mul(a.g, b.g),
                            mul(a.b, b.b),
                            mul(a.a, b.a),
                        ),
                        palette,
                        ..params
                    };
                    renderer.draw_model_matrix(SLOTS_CARD, &i.matrix, Material::VertexLit, &own);
                }
            } else {
                renderer.draw_model_instances_with(SLOTS_CARD, &instances, &palettes, &params);
            }
            renderer.end_frame();
        }
        *stats = renderer.stats();
        for (c, &(x, y)) in seen.iter_mut().zip(&probes) {
            match renderer.read_pixel(x, y) {
                Some(got) => *c = got,
                None => read = false,
            }
        }
    }
    let [separate, one_call] = stats;
    if separate.models != INSTANCES as u32 || separate.tris == 0 {
        return Err("instancing: the separate draws drew nothing");
    }
    let mut tnl = one_call.tnl;
    if tnl.instanced != 0 && tnl.instanced != INSTANCES as u32 {
        return Err("instancing: the instanced count is not one per copy");
    }
    tnl.instanced = separate.tnl.instanced;
    if one_call.models != separate.models || one_call.tris != separate.tris || tnl != separate.tnl {
        return Err("instancing: one call did different work than the separate draws");
    }
    if !read {
        return Ok(false);
    }
    if seen[0] != seen[1] {
        return Err("instancing: one call drew different pixels than the separate draws");
    }
    // The first card: its tint (red) over the call's, slot 0 of the card.
    let c = seen[1][48];
    if !(c.r > 150 && c.g < 120 && c.b < 120) {
        return Err("instancing: the first copy does not show its own tint");
    }
    Ok(true)
}

/// The AO probe panel, with ambient occlusion baked into its vertex colors.
pub const AO_PANEL: ModelId = ModelId::from_path("models/selftest_ao");
/// The same file baked without it.
pub const AO_PANEL_RAW: ModelId = ModelId::from_path("models/selftest_ao_raw");

/// Baked ambient occlusion ([`trino_core::ao`], the consoles' tier): the
/// white panel between two fins, unlit, baked with `ao` and without. With
/// it the panel is darker next to the fins and as white as without in its
/// middle, on every target — the occlusion is plain vertex color, drawn by
/// the RSP, the PICA200 or the PC alike. `Ok(false)` where `read_pixel`
/// is unsupported.
pub fn ambient_occlusion(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let params = ModelParams {
        shading: Shading::Unlit,
        double_sided: true,
        ..ModelParams::DEFAULT
    };
    // [edge, middle] of the panel, for the raw and the baked model.
    let mut seen = [[Color::BLACK; 2]; 2];
    for (k, model) in [AO_PANEL_RAW, AO_PANEL].into_iter().enumerate() {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.draw_model(model, &Transform3::default(), Material::VertexLit, &params);
            renderer.end_frame();
        }
        for (i, x) in [-0.46f32, 0.0].into_iter().enumerate() {
            let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
            let px = screen.x * 0.5 + v.x * focal / v.z;
            let py = screen.y * 0.5 - v.y * focal / v.z;
            let Some(c) = renderer.read_pixel(px as u32, py as u32) else {
                return Ok(false);
            };
            seen[k][i] = c;
        }
    }
    let luma = |c: Color| (c.r as i32 * 3 + c.g as i32 * 6 + c.b as i32) / 10;
    let [[raw_edge, raw_mid], [ao_edge, ao_mid]] = seen;
    if luma(raw_mid) < 200 || (luma(raw_edge) - luma(raw_mid)).abs() > 24 {
        return Err("ambient occlusion: the panel without AO is not evenly white");
    }
    if (luma(ao_mid) - luma(raw_mid)).abs() > 24 {
        return Err("ambient occlusion: the open middle of the panel darkened");
    }
    if luma(raw_edge) - luma(ao_edge) < 48 {
        return Err("ambient occlusion: no baked occlusion next to the fins");
    }
    if (ao_edge.r as i32 - ao_edge.b as i32).abs() > 24 {
        return Err("ambient occlusion: the occlusion tinted the panel");
    }
    Ok(true)
}

/// The shadows step's floor, post and ball (`[models.selftest_shadow_*]`).
pub const SHADOW_FLOOR: ModelId = ModelId::from_path("models/selftest_shadow_floor");
pub const SHADOW_POST: ModelId = ModelId::from_path("models/selftest_shadow_post");
pub const SHADOW_BALL: ModelId = ModelId::from_path("models/selftest_shadow_ball");
/// Where the post stands and the ball floats.
pub const SHADOW_POST_AT: Vec3 = Vec3::new(-1.2, 0.0, 0.6);
pub const SHADOW_BALL_AT: Vec3 = Vec3::new(0.6, 0.9, -0.6);

/// The shadows step's sun.
pub const SHADOW_SUN: Light = Light {
    dir: Vec3::new(0.45, -0.8, 0.35),
    ..DEFAULT_LIGHT
};

/// The shadows step's camera.
pub fn shadow_camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 4.2, -6.5),
        target: Vec3::new(0.0, 0.2, 0.4),
        fov_y: 50.0 * core::f32::consts::PI / 180.0,
    }
}

/// One frame of the shadows scene: the floor (the receiver), the post
/// (`ShadowCast::Mesh`) and the ball (`ShadowCast::Auto`), with
/// `shadows` (or none).
pub fn shadow_scene(renderer: &mut dyn Renderer, shadows: Option<&ShadowSettings>) {
    renderer.begin_frame(Color::rgb(140, 175, 220));
    renderer.set_camera(&shadow_camera());
    renderer.set_light(&SHADOW_SUN);
    renderer.set_shadows(shadows);
    renderer.set_shadow_receivers(&[ShadowPlane::horizontal(Vec3::ZERO, 4.0, 4.0)]);
    let at = |p: Vec3| Transform3 {
        position: p,
        ..Default::default()
    };
    let cast = |shadow| ModelParams {
        shadow,
        ..ModelParams::DEFAULT
    };
    renderer.draw_model(
        SHADOW_FLOOR,
        &at(Vec3::ZERO),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    renderer.draw_model(
        SHADOW_POST,
        &at(SHADOW_POST_AT),
        Material::VertexLit,
        &cast(ShadowCast::Mesh),
    );
    renderer.draw_model(
        SHADOW_BALL,
        &at(SHADOW_BALL_AT),
        Material::VertexLit,
        &cast(ShadowCast::auto(0.35)),
    );
    renderer.end_frame();
}

/// Shadows ([`trino_core::shadow`], ADR 0025), at each target's tier: the
/// post's shadow along the light (projected on the N64 and the 3DS, the
/// map on the PC) and the ball's (a blob straight down on the N64, along
/// the light elsewhere) darken the floor where they land; the open floor
/// stays as it was. `Ok(false)` where `read_pixel` is unsupported.
pub fn shadows(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = shadow_camera();
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    // Where the light's ray through `p` meets the floor.
    let l = SHADOW_SUN.dir;
    let ground = |p: Vec3| Vec3::new(p.x - l.x * p.y / l.y, 0.0, p.z - l.z * p.y / l.y);
    let post = ground(SHADOW_POST_AT + Vec3::new(0.0, 1.0, 0.0));
    let ball = if caps.shadow_tier == ShadowTier::Blob {
        Vec3::new(SHADOW_BALL_AT.x, 0.0, SHADOW_BALL_AT.z - 0.1)
    } else {
        ground(SHADOW_BALL_AT)
    };
    let open = Vec3::new(-2.6, 0.0, -2.0);
    let points = [post, ball, open];
    let mut seen = [[Color::BLACK; 3]; 2];
    let mut stats = trino_core::ShadowStats::default();
    for (k, on) in [false, true].into_iter().enumerate() {
        let settings = on.then_some(ShadowSettings::DEFAULT);
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            shadow_scene(renderer, settings.as_ref());
        }
        if on {
            stats = renderer.stats().shadows;
        }
        for (i, p) in points.into_iter().enumerate() {
            let v = view.transform_point(p);
            let px = screen.x * 0.5 + v.x * focal / v.z;
            let py = screen.y * 0.5 - v.y * focal / v.z;
            let Some(c) = renderer.read_pixel(px as u32, py as u32) else {
                return Ok(false);
            };
            seen[k][i] = c;
        }
    }
    let luma = |c: Color| (c.r as i32 * 3 + c.g as i32 * 6 + c.b as i32) / 10;
    let [off, on] = seen;
    let mut line = TextBuf::<160>::new();
    let _ = core::fmt::write(
        &mut line,
        format_args!(
            "TRINO_SHADOWS: tier={} blobs={} planar={} mapped={} tris={} post={}->{} ball={}->{} \
             open={}->{}\n",
            caps.shadow_tier.key(),
            stats.blobs,
            stats.planar,
            stats.mapped,
            stats.tris,
            luma(off[0]),
            luma(on[0]),
            luma(off[1]),
            luma(on[1]),
            luma(off[2]),
            luma(on[2]),
        ),
    );
    platform.log(line.as_str());
    if luma(off[0]) - luma(on[0]) < 30 {
        return Err("shadows: no shadow of the post on the floor");
    }
    if luma(off[1]) - luma(on[1]) < 30 {
        return Err("shadows: no shadow of the ball on the floor");
    }
    if (luma(off[2]) - luma(on[2])).abs() > 6 {
        return Err("shadows: the open floor changed");
    }
    if stats.blobs + stats.planar + stats.mapped < 2 {
        return Err("shadows: the frame counted no shadows");
    }
    Ok(true)
}

/// The far-blob golden's distances (`shadow_far_scene`), meters.
pub const SHADOW_FAR_AT: [f32; 6] = [4.0, 9.0, 18.0, 35.0, 60.0, 100.0];

/// The far-blob golden (the N64's `shadows_far`, ADR 0025): blobs on a
/// 256 m floor (the shadows floor 32 times over) out to 100 m from a low
/// camera, where the RDP's compressed z steps are coarsest: each lies on
/// the floor without fighting it. The casters are small balls on the
/// floor, each blob wider the farther it is (a pixel tall or more).
pub fn shadow_far_scene(renderer: &mut dyn Renderer) {
    renderer.begin_frame(Color::rgb(140, 175, 220));
    renderer.set_camera(&shadow_far_camera());
    renderer.set_light(&SHADOW_SUN);
    renderer.set_shadows(Some(&ShadowSettings {
        distance: 200.0,
        ..ShadowSettings::DEFAULT
    }));
    renderer.set_shadow_receivers(&[ShadowPlane::ground(0.0)]);
    renderer.draw_model(
        SHADOW_FLOOR,
        &Transform3 {
            scale: Vec3::new(32.0, 1.0, 32.0),
            ..Default::default()
        },
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    for (k, z) in SHADOW_FAR_AT.iter().enumerate() {
        renderer.draw_model(
            SHADOW_BALL,
            &Transform3 {
                position: shadow_far_ball(k),
                scale: Vec3::new(0.1, 0.1, 0.1),
                ..Default::default()
            },
            Material::VertexLit,
            &ModelParams {
                shadow: ShadowCast::Blob {
                    radius: 0.5 + z * 0.06,
                },
                ..ModelParams::DEFAULT
            },
        );
    }
    renderer.end_frame();
}

/// [`shadow_far_scene`]'s camera: eye height 3 m, looking down the floor.
pub fn shadow_far_camera() -> Camera3 {
    Camera3 {
        eye: Vec3::new(0.0, 3.0, -4.0),
        target: Vec3::new(0.0, 0.6, 40.0),
        fov_y: 50.0 * core::f32::consts::PI / 180.0,
    }
}

/// Where [`shadow_far_scene`]'s `k`th ball sits (alternating sides).
pub fn shadow_far_ball(k: usize) -> Vec3 {
    let z = SHADOW_FAR_AT[k];
    let side = if k.is_multiple_of(2) { -0.3 } else { 0.3 };
    Vec3::new(side * z * 0.2, 0.08, z)
}

/// Blob casters in [`shadow_crowd_scene`]: a football match's players.
pub const SHADOW_CROWD: usize = 22;

/// The blob cost scene (the N64's `shadow cost`): [`SHADOW_CROWD`] small
/// balls in a 6 x 4 grid over the shadows floor, each casting a blob
/// (`shadows` on) or not.
pub fn shadow_crowd_scene(renderer: &mut dyn Renderer, shadows: bool) {
    renderer.begin_frame(Color::rgb(140, 175, 220));
    renderer.set_camera(&shadow_camera());
    renderer.set_light(&SHADOW_SUN);
    renderer.set_shadows(shadows.then_some(&ShadowSettings::DEFAULT));
    renderer.set_shadow_receivers(&[ShadowPlane::horizontal(Vec3::ZERO, 4.0, 4.0)]);
    renderer.draw_model(
        SHADOW_FLOOR,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    let cast = ModelParams {
        shadow: ShadowCast::Blob { radius: 0.3 },
        ..ModelParams::DEFAULT
    };
    for i in 0..SHADOW_CROWD {
        let (x, z) = ((i % 6) as f32 - 2.5, (i / 6) as f32 - 1.5);
        renderer.draw_model(
            SHADOW_BALL,
            &Transform3 {
                position: Vec3::new(x * 1.1, 0.3, z * 1.2),
                scale: Vec3::new(0.5, 0.5, 0.5),
                ..Default::default()
            },
            Material::VertexLit,
            &cast,
        );
    }
    renderer.end_frame();
}

/// One frame of the water test scene: the sheet (`params`) over a red
/// floor from `camera`, at `time` with `ripples` (the `water` step, and the
/// 3DS's GPU-vs-CPU water frame).
pub fn water_scene(
    r: &mut dyn Renderer,
    camera: &Camera3,
    params: &ModelParams,
    time: f32,
    ripples: &[trino_core::Ripple],
) {
    let floor = ModelParams {
        shading: Shading::Unlit,
        tint: Color::rgba(255, 0, 0, 255),
        emissive: Color::rgba(255, 0, 0, 255),
        ..ModelParams::DEFAULT
    };
    r.begin_frame(Color::BLACK);
    r.set_camera(camera);
    r.set_light(&DEFAULT_LIGHT);
    r.set_water(time, ripples);
    r.draw_model(
        BLOCK,
        &Transform3 {
            position: Vec3::new(0.0, -1.0, 0.0),
            rotation: Vec3::ZERO,
            scale: Vec3::new(6.0, 1.0, 6.0),
        },
        Material::VertexLit,
        &floor,
    );
    r.draw_model(
        WATER_SHEET,
        &Transform3::default(),
        Material::VertexLit,
        params,
    );
    r.end_frame();
}

/// The water scene as the 3DS's GPU water path takes it (the whole sheet
/// past the near plane): a swell, a ripple, the two layers flowing, a
/// glint. `(camera, params, time, ripples)`.
pub fn gpu_water_scene() -> (Camera3, ModelParams, f32, [trino_core::Ripple; 1]) {
    let camera = Camera3 {
        eye: Vec3::new(0.0, 5.0, -5.5),
        target: Vec3::new(0.0, 0.0, 0.3),
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let params = ModelParams {
        texture: Some(WATER_TEX),
        detail: Some(DetailTexture::new(WATER_DETAIL, 1)),
        gloss: 200,
        water: Some(trino_core::Water {
            flow_speed: Vec2::new(0.2, 0.7),
            detail_flow: Vec2::new(0.5, -0.6),
            wave_amplitude: 0.15,
            wave_frequency: 0.7,
            ripple_responsiveness: 1.0,
        }),
        ..ModelParams::DEFAULT
    };
    let ripple = trino_core::Ripple {
        pos: Vec2::new(0.4, -0.3),
        age: 0.5,
        strength: 0.12,
    };
    (camera, params, 1.3, [ripple])
}

/// The water self-test's sheet: 4 × 4 m, 8 × 8 quads at y 0, translucent
/// blue (`[models.selftest_water]`).
pub const WATER_SHEET: ModelId = ModelId::from_path("models/selftest_water");
pub const WATER_TEX: SpriteId = SpriteId::from_path("sprites/tex_water");
pub const WATER_DETAIL: SpriteId = SpriteId::from_path("sprites/tex_ripples");

/// Water surfaces ([`trino_core::water`], ADR 0015): the sheet over a red
/// floor, seen from above at an angle, on every tier.
/// - **Translucent**: the floor shows through (red and blue both).
/// - **Swell**: a tall swell moves the surface (and its light).
/// - **Ripples**: a ripple's ring shows around its center (the consoles
///   draw it as a ring on the surface, the PC per pixel).
/// - **Flow**: later in time the layers have moved.
///
/// Each is two frames that must differ at probes over the sheet (or, for
/// the blend, one in between the two colors). `Ok(false)` where
/// `read_pixel` is unsupported.
pub fn water(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    use trino_core::{Ripple, Water};
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 3.0, -2.2),
        target: Vec3::new(0.0, 0.0, 0.2),
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let calm = Water {
        flow_speed: Vec2::new(0.2, 0.7),
        detail_flow: Vec2::new(0.5, -0.6),
        wave_amplitude: 0.0,
        wave_frequency: 0.5,
        ripple_responsiveness: 1.0,
    };
    let base = ModelParams {
        shading: Shading::Unlit,
        texture: Some(WATER_TEX),
        detail: Some(DetailTexture::new(WATER_DETAIL, 1)),
        water: Some(calm),
        ..ModelParams::DEFAULT
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let pixel = |world: Vec3| {
        let v = view.transform_point(world);
        (
            (screen.x * 0.5 + v.x * focal / v.z) as u32,
            (screen.y * 0.5 - v.y * focal / v.z) as u32,
        )
    };
    // Probes: a ring of 8 at 0.64 m around the middle (where the ripple
    // below spreads to), and its center.
    let mut probes = [(0u32, 0u32); 9];
    for (k, p) in probes.iter_mut().enumerate().take(8) {
        let a = k as f32 * core::f32::consts::PI / 4.0;
        *p = pixel(Vec3::new(0.64 * cos(a), 0.0, 0.64 * sin(a)));
    }
    probes[8] = pixel(Vec3::ZERO);
    let ripple = Ripple {
        pos: Vec2::new(0.0, 0.0),
        age: 0.4,
        strength: 0.12,
    };
    // One frame of the sheet over the floor; the probes' colors.
    let frame = |r: &mut dyn Renderer,
                 params: &ModelParams,
                 time: f32,
                 ripples: &[Ripple]|
     -> Option<[Color; 9]> {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            water_scene(r, &camera, params, time, ripples);
        }
        let mut out = [Color::BLACK; 9];
        for (o, &(x, y)) in out.iter_mut().zip(&probes) {
            *o = r.read_pixel(x, y)?;
        }
        Some(out)
    };
    let Some(still) = frame(renderer, &base, 0.0, &[]) else {
        return Ok(false);
    };
    // Translucent: the red floor through the blue water, in the middle.
    let c = still[8];
    if !(c.r > 40 && c.r < 230 && c.b > 50) {
        return Err("water: the floor does not show through the water");
    }
    let differ = |a: &[Color; 9], b: &[Color; 9]| {
        a.iter()
            .zip(b)
            .filter(|(x, y)| {
                let d = |p: u8, q: u8| (p as i32 - q as i32).abs();
                d(x.r, y.r) + d(x.g, y.g) + d(x.b, y.b) > 24
            })
            .count()
    };
    let lit = ModelParams {
        shading: Shading::Gouraud,
        ..base
    };
    let flat_lit = frame(renderer, &lit, 0.0, &[]).ok_or("water: read-back failed")?;
    let swell = Water {
        wave_amplitude: 0.4,
        wave_frequency: 0.7,
        ..calm
    };
    let swelling = frame(
        renderer,
        &ModelParams {
            water: Some(swell),
            ..lit
        },
        0.0,
        &[],
    )
    .ok_or("water: read-back failed")?;
    if differ(&flat_lit, &swelling) < 3 {
        return Err("water: the swell did not move the surface");
    }
    let rippled = frame(renderer, &base, 0.0, &[ripple]).ok_or("water: read-back failed")?;
    if differ(&still, &rippled) < 3 {
        return Err("water: a ripple left no ring");
    }
    // The flow on high-contrast layers (the quadrants and the white/gray
    // detail): the texture half a repeat on, then the detail layer alone.
    let quads = ModelParams {
        texture: Some(QUADRANTS16),
        detail: Some(DetailTexture::new(DETAIL, 0)),
        water: Some(Water {
            flow_speed: Vec2::new(0.0, 0.5),
            detail_flow: Vec2::new(0.0, 0.0),
            ..calm
        }),
        ..base
    };
    let start = frame(renderer, &quads, 0.0, &[]).ok_or("water: read-back failed")?;
    let later = frame(renderer, &quads, 1.0, &[]).ok_or("water: read-back failed")?;
    if differ(&start, &later) < 3 {
        return Err("water: the texture did not flow");
    }
    let detail_flows = ModelParams {
        water: Some(Water {
            flow_speed: Vec2::new(0.0, 0.0),
            detail_flow: Vec2::new(0.5, 0.0),
            ..calm
        }),
        ..quads
    };
    let start = frame(renderer, &detail_flows, 0.0, &[]).ok_or("water: read-back failed")?;
    let later = frame(renderer, &detail_flows, 1.0, &[]).ok_or("water: read-back failed")?;
    if differ(&start, &later) < 3 {
        return Err("water: the detail layer did not flow");
    }
    Ok(true)
}

/// Local lights ([`trino_core::light`]): the white slot card lit only by
/// a dim ambient, then with a red point light in front of its left column.
/// The left column turns red, the right one (out of reach) stays gray, on
/// every tier (per pixel on PC, per vertex on the consoles); the lit frame
/// draws nothing on the N64's RSP (`RspReject::LocalLight`), and the next
/// frame without lights is the unlit one again (cleared at
/// `begin_frame`). `Ok(false)` where `read_pixel` is unsupported.
pub fn local_lights(renderer: &mut dyn Renderer) -> Result<bool, &'static str> {
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    // Ambient only: the global light travels away from the viewer.
    let dim = Light {
        dir: Vec3::new(0.0, 0.0, -1.0),
        ambient: 0.35,
        ..DEFAULT_LIGHT
    };
    let lamp = LocalLight::point(Vec3::new(-0.4, 0.0, -0.25), Color::rgb(255, 0, 0), 0.6)
        .with_intensity(3.0);
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * screen.y * 0.5;
    let probe = |r: &mut dyn Renderer, x: f32| {
        let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
        let px = screen.x * 0.5 + v.x * focal / v.z;
        let py = screen.y * 0.5 - v.y * focal / v.z;
        r.read_pixel(px as u32, py as u32)
    };
    let mut rsp_lit = 0;
    let mut seen = [[Color::BLACK; 2]; 3];
    for (pass, lights) in [&[][..], &[lamp][..], &[][..]].into_iter().enumerate() {
        // Several identical frames: the 3DS LCD is double-buffered.
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&dim);
            renderer.set_local_lights(lights);
            renderer.draw_model(
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &ModelParams::DEFAULT,
            );
            renderer.end_frame();
        }
        if pass == 1 {
            rsp_lit = renderer.stats().tnl.rsp_draws;
        }
        for (k, x) in [-0.4f32, 0.4].into_iter().enumerate() {
            let Some(c) = probe(renderer, x) else {
                return Ok(false);
            };
            seen[pass][k] = c;
        }
    }
    let gray = |c: Color| c.r.abs_diff(c.g) <= 24 && c.g.abs_diff(c.b) <= 24 && c.g > 40;
    let [off, on, after] = seen;
    if !(gray(off[0]) && gray(off[1])) {
        return Err("local lights: the unlit card is not gray");
    }
    if !(on[0].r as i32 >= on[0].g as i32 + 40 && on[0].r as i32 >= off[0].r as i32 + 30) {
        return Err("local lights: the lit column did not turn red");
    }
    if !gray(on[1]) {
        return Err("local lights: a light reached past its radius");
    }
    if rsp_lit != 0 {
        return Err("local lights: a lit draw stayed on the N64 RSP");
    }
    if after != off {
        return Err("local lights: lights survived begin_frame");
    }
    Ok(true)
}

/// The boar pre-rendered from 8 yaws (`cargo xtask impostor bake
/// selftest-boar`): its `TIMP` table; the atlas is
/// `sprites/impostor-selftest-boar` (CI4 within TMEM on the N64).
pub const BOAR_IMPOSTOR: BlobId = BlobId::from_path("blobs/impostor-selftest-boar");

/// Impostors: the baked table parses on the target, the view seen from the
/// side is the side view on every target (the core's deterministic yaw),
/// the draw is one billboard of exactly 2 triangles, and the boar's body
/// shows at its center. `Ok(false)` where `read_pixel` is unsupported.
pub fn impostor(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    let bytes = platform
        .blob(BOAR_IMPOSTOR)
        .ok_or("impostor: table blob missing")?;
    let imp = Impostor::parse(bytes).map_err(|_| "impostor: table does not parse")?;
    if imp.views != 8 {
        return Err("impostor: expected 8 views");
    }
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    // From the boar's right (+X), 6 m away, level with its center.
    let center = imp.center;
    let eye = Vec3::new(center.x + 6.0, center.y, center.z);
    if imp.view_for(&trino_core::Mat34::IDENTITY, eye) != 2 {
        return Err("impostor: the side camera picked the wrong view");
    }
    let camera = Camera3 {
        eye,
        target: center,
        fov_y: 0.6,
    };
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        imp.draw(renderer, &trino_core::Mat34::IDENTITY, eye, Color::WHITE);
        renderer.end_frame();
    }
    let stats = renderer.stats();
    if stats.tris != 2 || stats.sprites != 1 {
        return Err("impostor: not one billboard of 2 triangles");
    }
    let Some(got) = renderer.read_pixel(screen.x as u32 / 2, screen.y as u32 / 2) else {
        return Ok(false);
    };
    // The boar is brown: lit, and redder than it is blue.
    if (got.r as u32 + got.g as u32 + got.b as u32) < 60 || got.r <= got.b {
        return Err("impostor: the center pixel is not the boar");
    }
    Ok(true)
}

/// Sparks baked per platform (`[particles.selftest-sparks]`): the cap is
/// 24 on PC, 8 on the N64 and 16 on the 3DS.
pub const SPARKS: BlobId = BlobId::from_path("particles/selftest-sparks");

/// FNV-1a over the particles' exact positions.
fn particle_hash<const N: usize>(s: &ParticleSystem<N>) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for p in s.particles() {
        for v in [p.pos.x, p.pos.y, p.pos.z] {
            for b in v.to_bits().to_le_bytes() {
                h ^= b as u32;
                h = h.wrapping_mul(0x0100_0193);
            }
        }
    }
    h
}

/// Particle effects: the baked effect parses on the target with this
/// platform's cap, a burst never passes it, the same seed gives the same
/// particles (and another seed others), and drawing them is one billboard
/// per particle. Logs the cap (`TRINO_STEP:particles cap N`).
#[inline(never)]
pub fn particles(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<(), &'static str> {
    let bytes = platform
        .blob(SPARKS)
        .ok_or("particles: effect blob missing")?;
    let fx = Effect::parse(bytes).map_err(|_| "particles: effect does not parse")?;
    if fx.max == 0 || fx.max > 24 {
        return Err("particles: the platform cap is not baked in");
    }
    let run = |seed: u32| {
        let mut s: ParticleSystem<32> = ParticleSystem::from_effect(&fx, seed);
        s.burst(100);
        let first = s.len();
        let mut most = first;
        for _ in 0..45 {
            s.update(1.0 / 30.0);
            most = most.max(s.len());
        }
        (s, first, most)
    };
    let (a, first, most) = run(7);
    let cap = fx.max as usize;
    if first != cap || most > cap {
        return Err("particles: the cap does not hold");
    }
    let (b, ..) = run(7);
    let (c, ..) = run(8);
    if particle_hash(&a) != particle_hash(&b) {
        return Err("particles: the same seed gave different particles");
    }
    if particle_hash(&a) == particle_hash(&c) {
        return Err("particles: another seed gave the same particles");
    }
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 1.0, -6.0),
        target: Vec3::new(0.0, 1.0, 0.0),
        fov_y: 1.0,
    });
    renderer.set_light(&DEFAULT_LIGHT);
    a.draw(renderer);
    renderer.end_frame();
    if renderer.stats().sprites as usize != a.len() {
        return Err("particles: not one billboard per particle");
    }
    let mut line: TextBuf<48> = TextBuf::new();
    let _ =
        core::fmt::Write::write_fmt(&mut line, format_args!("TRINO_STEP:particles cap {cap}\n"));
    platform.log(line.as_str());
    Ok(())
}

/// The baked portal/cell demo (`scenes/interior.scene.ron`).
pub const INTERIOR: BlobId = BlobId::from_path("scenes/interior");

/// Portal visibility end to end: the interior scene baked by the pipeline,
/// parsed on the target, cells and portals rebuilt, masks computed from
/// two cameras in the first room, and the draw counts the backend saw.
pub fn portals(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<(), &'static str> {
    let bytes = platform
        .blob(INTERIOR)
        .ok_or("interior scene blob missing")?;
    let baked = BakedScene::parse(bytes).map_err(|_| "interior scene does not parse")?;
    let cells: Cells<4> = Cells::from_nodes(baked.nodes());
    if (cells.cell_count(), cells.portal_count()) != (3, 2) {
        return Err("interior: expected 3 cells and 2 portals");
    }
    let mut world: EntityStore<32> = EntityStore::new();
    world.spawn_iter(baked.nodes());
    let caps = *renderer.caps();
    let screen = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let eye = Vec3::new(1.0, 1.6, 4.0);
    let mut models = |target: Vec3, cull: bool| {
        let camera = Camera3 {
            eye,
            target,
            fov_y: 1.05,
        };
        let mask = if cull {
            cells.visible(&camera, screen)
        } else {
            u64::MAX
        };
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        world.draw_in(renderer, &trino_core::Mat34::IDENTITY, 0.0, eye, mask);
        renderer.end_frame();
        (mask & 0b111, renderer.stats().models)
    };
    // Down the doorways: every room is visible, nothing is culled.
    let east = Vec3::new(23.0, 1.4, 4.0);
    let (mask, through) = models(east, true);
    if mask != 0b111 || through != models(east, false).1 {
        return Err("portals: the doorway chain should show all three rooms");
    }
    // Facing the back wall: rooms B and C (3 models each) are skipped.
    let west = Vec3::new(-10.0, 1.6, 4.0);
    let (mask, away) = models(west, true);
    let all = models(west, false).1;
    if mask != 0b001 || all - away != 6 {
        return Err("portals: facing away should cull the two far rooms");
    }
    Ok(())
}

/// The hardware-tier probe scene (`[scenes.selftest-tier]`, issue #54): one
/// card, drawn with [`TIER_CARD`] in the expanded tiers' variant.
pub const TIER_SCENE: BlobId = BlobId::from_path("scenes/selftest-tier");
/// A model only the expanded tiers' data has (`platforms = ["n64_expanded",
/// "n3ds_new"]`): in the ROM / app, loaded on that hardware only.
pub const TIER_CARD: ModelId = ModelId::from_path("models/selftest_tier_card");
/// Sparks capped 4, and 6 on the expanded tiers
/// (`[particles.selftest-tier-sparks]`).
pub const TIER_SPARKS: BlobId = BlobId::from_path("particles/selftest-tier-sparks");

/// What [`hw_tier`] found: the hardware's tier and the tier the loaded data
/// was resolved for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TierReport {
    /// [`Platform::hw_tier`] (the renderer's caps agree).
    pub tier: HwTier,
    /// The tier the probe scene, the sparks and the tier-only model were
    /// loaded for: the hardware's on a console, the base one on the PC and
    /// the Web, whose one bake serves every simulated tier.
    pub data: HwTier,
    pub ram_bytes: u32,
    pub cpu_cores: u8,
    /// The tier sparks' baked cap.
    pub sparks: u16,
}

/// The hardware tier (issue #54, ADR 0020): the platform's answer and the
/// renderer's caps agree and are that tier's table (an expanded tier adds
/// RAM or cores, never takes them); the data the build loaded is one
/// tier's throughout — the probe scene's card, the tier sparks' cap and the
/// tier-only model are all base or all expanded — and never the expanded
/// tier's on base hardware. `cargo xtask test n64|3ds` then holds `data` to
/// what the emulator ran (the `TRINO_STEP:hw tier` line).
pub fn hw_tier(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<TierReport, &'static str> {
    let caps = *renderer.caps();
    let tier = platform.hw_tier();
    if caps.hw_tier != tier {
        return Err("hw tier: the renderer's caps and the platform disagree");
    }
    let table = caps.with_tier(tier);
    if (table.ram_bytes, table.cpu_cores, table.cpu_mhz)
        != (caps.ram_bytes, caps.cpu_cores, caps.cpu_mhz)
        || (table.crowd_percent, table.particle_percent)
            != (caps.crowd_percent, caps.particle_percent)
    {
        return Err("hw tier: the caps are not the tier's table");
    }
    let base = caps.base();
    if caps.ram_bytes < base.ram_bytes || caps.cpu_cores < base.cpu_cores {
        return Err("hw tier: an expanded tier with less than the base console");
    }
    // The probe scene: the card it draws says which tier it resolved for.
    let bytes = platform
        .blob(TIER_SCENE)
        .ok_or("hw tier: probe scene blob missing")?;
    let scene = BakedScene::parse(bytes).map_err(|_| "hw tier: probe scene does not parse")?;
    let data = match scene.find("probe").and_then(|n| n.model) {
        Some(CARD) => HwTier::Base,
        Some(TIER_CARD) => HwTier::Expanded,
        _ => return Err("hw tier: the probe draws neither card"),
    };
    if data.is_expanded() && !tier.is_expanded() {
        return Err("hw tier: the expanded tier's data on base hardware");
    }
    let sparks = platform
        .blob(TIER_SPARKS)
        .ok_or("hw tier: tier sparks blob missing")?;
    let fx = Effect::parse(sparks).map_err(|_| "hw tier: tier sparks do not parse")?;
    if fx.max != if data.is_expanded() { 6 } else { 4 } {
        return Err("hw tier: the sparks' cap is another tier's");
    }
    // The tier-only model draws exactly where the expanded data is loaded.
    renderer.begin_frame(Color::BLACK);
    renderer.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    });
    renderer.set_light(&DEFAULT_LIGHT);
    renderer.draw_model(
        TIER_CARD,
        &Transform3::default(),
        Material::VertexLit,
        &ModelParams::DEFAULT,
    );
    renderer.end_frame();
    if (renderer.stats().models == 1) != data.is_expanded() {
        return Err("hw tier: the tier-only model is loaded on the wrong tier");
    }
    Ok(TierReport {
        tier,
        data,
        ram_bytes: caps.ram_bytes,
        cpu_cores: caps.cpu_cores,
        sparks: fx.max,
    })
}

/// `TRINO_STEP:hw tier expanded ram=8M cores=1 data=expanded sparks=6`.
pub fn log_tier(platform: &mut dyn Platform, t: &TierReport) {
    let mut line: TextBuf<96> = TextBuf::new();
    let ram = if t.ram_bytes == u32::MAX {
        0
    } else {
        t.ram_bytes >> 20
    };
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:hw tier {} ram={ram}M cores={} data={} sparks={}\n",
            t.tier.key(),
            t.cpu_cores,
            t.data.key(),
            t.sparks
        ),
    );
    platform.log(line.as_str());
}

/// What [`quality`] found (issue #55).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct QualityReport {
    pub preset: QualityPreset,
    /// The console's own preset: no host reports one
    /// ([`Platform::quality_info`] is `None`).
    pub fixed: bool,
    /// [`Renderer::lod_bias`] × 100.
    pub lod_pct: u16,
    /// The sparks' cap after the quality's particle scale.
    pub particles: usize,
}

/// The quality preset (issue #55). A console's is fixed and scales nothing:
/// `Low` on the N64 (with or without the Expansion Pak) and the 3DS,
/// `Medium` on the New 3DS ([`QualitySettings::console`]); `set_quality`
/// changes nothing, no preset table is offered, the renderer's LOD bias is
/// 1 and the particle caps stay the baked ones. A host that manages the
/// quality (the PC's native profile, the Web) reports it and offers its
/// presets; its renderer applies the LOD bias, its particle scale holds.
pub fn quality(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<QualityReport, &'static str> {
    let q = platform.quality();
    if q.clamped() != q {
        return Err("quality: settings out of their ranges");
    }
    let bytes = platform
        .blob(SPARKS)
        .ok_or("quality: effect blob missing")?;
    let fx = Effect::parse(bytes).map_err(|_| "quality: effect does not parse")?;
    let mut sparks: ParticleSystem<32> = ParticleSystem::from_effect(&fx, 3);
    sparks.scale_limit(q.particle_scale);
    sparks.burst(100);
    if sparks.len() != q.particle_cap(fx.max as usize).min(32) {
        return Err("quality: the particle scale does not hold");
    }
    let fixed = platform.quality_info().is_none();
    let bias = renderer.lod_bias();
    if fixed {
        if q != QualitySettings::console(renderer.caps()) {
            return Err("quality: not the console's fixed preset");
        }
        if q.render_scale != 1.0 || q.dynamic_resolution.is_some() || q.particle_scale != 1.0 {
            return Err("quality: a console's preset scales something");
        }
        platform.set_quality(QualitySettings::ULTRA);
        if platform.quality() != q {
            return Err("quality: set_quality changed a console's preset");
        }
        if platform.quality_preset(QualityPreset::Low).is_some() {
            return Err("quality: a console offers presets");
        }
        if bias != 1.0 {
            return Err("quality: a console's renderer biases its LODs");
        }
    } else {
        if platform.quality_preset(QualityPreset::Low).is_none() {
            return Err("quality: the host offers no presets");
        }
        if (bias - q.lod_bias).abs() > 1e-3 {
            return Err("quality: the renderer's LOD bias is not the quality's");
        }
    }
    Ok(QualityReport {
        preset: q.preset,
        fixed,
        lod_pct: (bias * 100.0 + 0.5) as u16,
        particles: sparks.len(),
    })
}

/// `TRINO_STEP:quality low fixed lod=100 particles=4`.
pub fn log_quality(platform: &mut dyn Platform, q: &QualityReport) {
    let mut line: TextBuf<96> = TextBuf::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:quality {} {} lod={} particles={}\n",
            q.preset.key(),
            if q.fixed { "fixed" } else { "hosted" },
            q.lod_pct,
            q.particles
        ),
    );
    platform.log(line.as_str());
}

// ---------------------------------------------------------------------------
// Asset groups (issue #56): `Platform::load_group` / `unload_group`.

/// `[groups.selftest_level]`: the card, its texture and a blob.
pub const LEVEL: trino_core::GroupId = trino_core::GroupId::from_name("selftest_level");
/// `[groups.selftest_level2]`: the same texture (shared), a sound and a
/// blob of its own (the N64 compresses it at level 2: aPLib).
pub const LEVEL2: trino_core::GroupId = trino_core::GroupId::from_name("selftest_level2");
pub const LEVEL_TILE: SpriteId = SpriteId::from_path("sprites/selftest_level_tile");
pub const LEVEL_CARD: ModelId = ModelId::from_path("models/selftest_level_card");
pub const LEVEL_BLOB: BlobId = BlobId::from_path("blobs/selftest_level");
pub const LEVEL_BLOB_CONTENT: &[u8] = b"trino-group-ok\n";
pub const LEVEL2_BLOB: BlobId = BlobId::from_path("blobs/selftest_level2");

/// What the `asset groups` check drives: the backends and the host's loader
/// (`trino_runner_pc::GroupLoader::pump` on the PC, the console hosts' own
/// pump), one call per frame, before the game's update.
pub trait GroupHost {
    fn parts(&mut self) -> (&mut dyn Renderer, &mut dyn Platform);
    fn pump(&mut self);
    /// How far the heap may sit above its baseline after an unload: what
    /// the backend's own bookkeeping grows by while frames go by (the
    /// consoles: none to speak of; wgpu's trackers grow a few KB per
    /// thousand frames even without loads).
    fn heap_tolerance(&self) -> u32 {
        8 * 1024
    }
}

/// Frames a group may take to load before the check gives up.
const LOAD_FRAMES: u32 = 600;

/// One frame as a game's loading screen draws it: a bar of `percent`, the
/// host's loader step first.
fn loading_frame(host: &mut dyn GroupHost, percent: u8) {
    host.pump();
    let (renderer, _) = host.parts();
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    renderer.begin_frame(Color::BLACK);
    let full = w * 0.6;
    renderer.fill_rect(
        Rect::new(w * 0.2, h * 0.5, full, 8.0),
        Color::rgb(60, 60, 60),
    );
    let done = full * percent as f32 / 100.0;
    renderer.fill_rect(
        Rect::new(w * 0.2, h * 0.5, done, 8.0),
        Color::rgb(80, 220, 120),
    );
    renderer.end_frame();
}

/// Load `group` behind a loading bar: progress never goes back, and it ends
/// `Ready` within [`LOAD_FRAMES`]. Returns the frames it took.
fn load_until_ready(
    host: &mut dyn GroupHost,
    group: trino_core::GroupId,
) -> Result<u32, &'static str> {
    let mut last = 0u8;
    for frame in 0..LOAD_FRAMES {
        let state = host.parts().1.load_group(group);
        match state {
            trino_core::LoadState::Ready => return Ok(frame),
            trino_core::LoadState::Failed => return Err("asset groups: a group failed to load"),
            trino_core::LoadState::Loading { progress } => {
                if progress < last {
                    return Err("asset groups: load progress went back");
                }
                last = progress;
                loading_frame(host, progress);
            }
            trino_core::LoadState::Unloaded => {
                return Err("asset groups: load_group left the group unloaded");
            }
        }
    }
    Err("asset groups: a group never got ready")
}

/// Draw the level's tile (8x: 128x128, its four quadrants) and card; the
/// frame's `not_loaded` count and, where `read_pixel` works, the tile's
/// top-left quadrant (red) or `None`.
fn draw_level(host: &mut dyn GroupHost) -> (u32, Option<Color>) {
    let (renderer, _) = host.parts();
    let caps = *renderer.caps();
    let (w, h) = (caps.screen_width as f32, caps.screen_height as f32);
    let tile = SpriteParams {
        scale: Vec2::new(8.0, 8.0),
        ..SpriteParams::DEFAULT
    };
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -3.0),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let mut not_loaded = 0;
    // Several identical frames: the 3DS LCD is double-buffered.
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&camera);
        renderer.set_light(&DEFAULT_LIGHT);
        renderer.draw_model(
            LEVEL_CARD,
            &Transform3 {
                position: Vec3::new(1.2, 0.0, 0.0),
                ..Transform3::default()
            },
            Material::VertexLit,
            &ModelParams::DEFAULT,
        );
        renderer.draw_sprite(LEVEL_TILE, Vec2::new(w * 0.5 - 64.0, h * 0.5 - 64.0), &tile);
        renderer.end_frame();
        not_loaded = renderer.stats().not_loaded;
    }
    let probe = renderer.read_pixel((w * 0.5 - 32.0) as u32, (h * 0.5 - 32.0) as u32);
    (not_loaded, probe)
}

fn is_red(c: Option<Color>) -> bool {
    c.is_none_or(|c| c.r > 200 && c.g < 60 && c.b < 60)
}

/// The heap a backend reports (0 where it doesn't count).
fn heap_used(host: &mut dyn GroupHost) -> u32 {
    host.parts().1.memory().heap_used
}

/// What the `asset groups` step measured, for its log line.
#[derive(Clone, Copy, Debug, Default)]
pub struct GroupReport {
    /// Frames the first load took (behind the loading bar).
    pub load_frames: u32,
    /// Bytes the level's assets took (`MemoryReport::assets`).
    pub level_bytes: u32,
    /// Heap after the warm-up unload, and the most it was above that after
    /// any later unload.
    pub heap_baseline: u32,
    pub heap_drift: u32,
    /// Load/unload cycles of the soak, and its frames.
    pub cycles: u32,
    pub soak_frames: u32,
}

/// Asset groups end to end: an unloaded group's assets don't draw (counted
/// as `not_loaded`, never a crash) and its blob isn't there; `load_group`
/// reaches `Ready` behind a loading bar with progress that never goes back;
/// its sprite, card and blob are there and its bytes count in `memory()`;
/// a texture two groups share stays while either is loaded; an unloaded
/// group frees its memory (the heap back within the host's tolerance of the
/// warm-up baseline); an unknown group fails. Then `soak_frames` frames of
/// load/unload cycles (fragmentation): the heap never drifts from the
/// baseline and the group still loads at the end.
pub fn asset_groups(
    host: &mut dyn GroupHost,
    soak_frames: u32,
) -> Result<GroupReport, &'static str> {
    let tolerance = host.heap_tolerance();
    let mut report = GroupReport::default();
    let unknown = trino_core::GroupId::from_name("selftest_no_such_group");
    {
        let (_, platform) = host.parts();
        if platform.load_group(trino_core::GroupId::BOOT) != trino_core::LoadState::Ready {
            return Err("asset groups: boot is not ready");
        }
        if platform.group_state(LEVEL) != trino_core::LoadState::Unloaded {
            return Err("asset groups: the level group is not in this build (or loaded at boot)");
        }
        if platform.group_blob(LEVEL_BLOB).is_some() {
            return Err("asset groups: an unloaded group's blob is there");
        }
        if platform.load_group(unknown) != trino_core::LoadState::Failed {
            return Err("asset groups: an unknown group did not fail");
        }
    }
    let (missing, _) = draw_level(host);
    if missing < 2 {
        return Err("asset groups: an unloaded group's draws were not counted as not_loaded");
    }
    let before = host.parts().1.memory();
    report.load_frames = load_until_ready(host, LEVEL)?;
    let after = host.parts().1.memory();
    report.level_bytes = after.assets().saturating_sub(before.assets());
    if report.level_bytes == 0 {
        return Err("asset groups: the loaded group's assets are not counted in memory()");
    }
    if host.parts().1.group_blob(LEVEL_BLOB) != Some(LEVEL_BLOB_CONTENT) {
        return Err("asset groups: the group's blob is missing or wrong");
    }
    let (missing, probe) = draw_level(host);
    if missing != 0 {
        return Err("asset groups: a ready group's assets did not draw");
    }
    if !is_red(probe) {
        return Err("asset groups: the group's sprite drew the wrong color");
    }
    // The shared texture survives the first group's unload.
    load_until_ready(host, LEVEL2)?;
    if host.parts().1.group_blob(LEVEL2_BLOB) != Some(LEVEL_BLOB_CONTENT) {
        return Err("asset groups: the second group's blob is missing or wrong");
    }
    host.parts().1.unload_group(LEVEL);
    if host.parts().1.group_state(LEVEL) != trino_core::LoadState::Unloaded {
        return Err("asset groups: unload_group left the group loaded");
    }
    host.pump();
    if host.parts().1.group_blob(LEVEL_BLOB).is_some() {
        return Err("asset groups: an unloaded group's blob is still there");
    }
    let (missing, probe) = draw_level(host);
    if missing != 1 || !is_red(probe) {
        return Err("asset groups: a texture both groups hold went with the first unload");
    }
    host.parts().1.unload_group(LEVEL2);
    host.pump();
    let (missing, _) = draw_level(host);
    if missing < 2 {
        return Err("asset groups: the last unload left the shared texture loaded");
    }
    let freed = host.parts().1.memory();
    if freed.assets() != before.assets() {
        return Err("asset groups: memory() still counts an unloaded group's assets");
    }
    // Warm-up done: everything the backends keep around exists now.
    loading_frame(host, 100);
    report.heap_baseline = heap_used(host);
    // The soak: what a game does between levels, one frame at a time (each
    // frame pumps the loader and draws, as a loading screen would).
    let mut cycle_frames = 0u32;
    while cycle_frames < soak_frames {
        let state = host.parts().1.load_group(LEVEL);
        match state {
            trino_core::LoadState::Ready => {
                host.parts().1.unload_group(LEVEL);
                loading_frame(host, 100);
                report.cycles += 1;
                let drift = heap_used(host).saturating_sub(report.heap_baseline);
                report.heap_drift = report.heap_drift.max(drift);
                if drift > tolerance {
                    return Err("asset groups: the heap grows over load/unload cycles");
                }
            }
            trino_core::LoadState::Loading { progress } => loading_frame(host, progress),
            _ => return Err("asset groups: a group failed during the soak"),
        }
        cycle_frames += 1;
    }
    report.soak_frames = cycle_frames;
    // After the soak (a fragmented heap would fail here) the group loads.
    load_until_ready(host, LEVEL)?;
    host.parts().1.unload_group(LEVEL);
    loading_frame(host, 100);
    if heap_used(host).saturating_sub(report.heap_baseline) > tolerance {
        return Err("asset groups: the heap did not return to its baseline after unload");
    }
    Ok(report)
}

/// The `asset groups` step's log line.
pub fn log_groups(platform: &mut dyn Platform, r: &GroupReport) {
    let mut line = TextBuf::<160>::new();
    let _ = core::fmt::Write::write_fmt(
        &mut line,
        format_args!(
            "TRINO_STEP:asset groups ok load_frames={} level_bytes={} heap_baseline={} \
             heap_drift={} cycles={} soak_frames={}\n",
            r.load_frames, r.level_bytes, r.heap_baseline, r.heap_drift, r.cycles, r.soak_frames
        ),
    );
    platform.log(line.as_str());
}

/// The stripe pattern `sprites/selftest_quadrants` shows under a
/// color-vision mode (`cvd_icon` in the manifest, and on `ui/selftest`'s
/// first button).
pub const CVD_STRIPES: SpriteId = SpriteId::from_path("sprites/cvd_stripes");

/// Audio that only counts what reaches it: the caption wrapper must pass
/// every call through.
struct CountingAudio {
    sounds: u32,
    music: u32,
}

impl trino_core::Audio for CountingAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {
        self.sounds += 1;
    }
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {
        self.music += 1;
    }
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Accessibility (issue #60): the platform keeps the player's settings and
/// the 3-byte save form round-trips; the baked `access/data` holds the
/// semantic colors, the captions and the icons; a slotted card drawn with
/// the deuteranopia palette shows the alternatives (the same draw as the
/// base palette: no shader); the caption queue fills through the audio
/// wrapper and draws a dark box with text over the scene (on the touch
/// screen with `SecondScreen`, where there is one); the UI at 200 % text
/// keeps its texts in their boxes, the high-contrast theme fills the
/// focused button with its yellow and a color-vision mode adds the
/// button's icon; a high-contrast outline rings a card in its color; the
/// flash guard keeps a 10 Hz strobe to 3 flashes a second, reduced motion
/// damps a shake, hold-to-toggle latches. `Ok(false)`
/// after the non-pixel checks where `read_pixel` is unsupported.
#[inline(never)]
pub fn accessibility(
    renderer: &mut dyn Renderer,
    platform: &mut dyn Platform,
) -> Result<bool, &'static str> {
    use trino_core::access::{CaptionMode, MAX_FLASHES_PER_SECOND};
    use trino_core::ui::{NoStrings, Ui, UiEnv, UiScreen};
    use trino_core::{
        AccessBlob, AccessSettings, Audio, CameraFx, CaptionPlace, CaptionStyle, Captions, CvdMode,
        FlashMonitor, HoldToggle, ScreenFlash,
    };

    // 1. Settings: the save form, the platform round trip.
    let want = AccessSettings {
        cvd: CvdMode::Deuteranopia,
        text_scale: 200,
        high_contrast: true,
        captions: CaptionMode::All,
        reduce_motion: true,
        flash_guard: true,
        hold_to_toggle: true,
    };
    if AccessSettings::unpack(want.pack()) != Some(want) {
        return Err("accessibility: the packed settings do not round-trip");
    }
    let saved = platform.access();
    platform.set_access(want);
    let kept = platform.access();
    platform.set_access(saved);
    if kept != want || platform.access() != saved {
        return Err("accessibility: the platform does not keep its settings");
    }

    // 2. The baked data.
    let mut blob = AccessBlob::new();
    let data = *blob
        .get(platform)
        .ok_or("accessibility: blob access/data missing or broken")?;
    let rgb = |r, g, b| Color::rgb(r, g, b);
    let (a_base, b_base) = (rgb(220, 40, 40), rgb(40, 160, 60));
    let (a_deut, b_deut) = (rgb(240, 160, 0), rgb(40, 90, 220));
    if data.color("selftest.team_a", CvdMode::Off) != Some(a_base)
        || data.color("selftest.team_b", CvdMode::Deuteranopia) != Some(b_deut)
        || data.color("selftest.team_a", CvdMode::Tritanopia) != Some(a_base)
    {
        return Err("accessibility: a semantic color has the wrong value");
    }
    if data.icon(QUADRANTS) != Some(CVD_STRIPES) {
        return Err("accessibility: the quadrants' color-vision icon is missing");
    }
    let coin = trino_core::SoundId::from_path("sounds/coin");
    let win = trino_core::SoundId::from_path("sounds/win");
    match (data.caption(coin.0), data.caption(win.0)) {
        (Some(c), Some(w)) if c.key == "cap.coin" && !c.important && w.important => {}
        _ => return Err("accessibility: the sounds' captions are wrong"),
    }

    // 3. Motion, flashes, held input (no renderer).
    let mut flash = ScreenFlash::new();
    let guarded = AccessSettings::DEFAULT;
    let open = AccessSettings {
        flash_guard: false,
        ..guarded
    };
    let mut shown = 0;
    for _ in 0..10 {
        shown += flash.trigger(Color::WHITE, 0.05, &guarded) as usize;
        flash.update(0.1);
    }
    if shown != MAX_FLASHES_PER_SECOND || flash.dropped != 7 {
        return Err("accessibility: the flash guard let a 10 Hz strobe through");
    }
    let mut free = ScreenFlash::new();
    let free_shown = (0..10)
        .map(|_| {
            let s = free.trigger(Color::WHITE, 0.05, &open);
            free.update(0.1);
            s as usize
        })
        .sum::<usize>();
    if free_shown != 10 || flash.current().a > trino_core::access::motion::GUARDED_ALPHA {
        return Err("accessibility: flashes without the guard were dropped");
    }
    let mut monitor = FlashMonitor::new();
    let strobe = (0..60).any(|f| monitor.frame(f as f32 / 60.0, ((f / 3) % 2) as f32));
    let mut calm = FlashMonitor::new();
    let slow = (0..120).any(|f| calm.frame(f as f32 / 60.0, ((f / 30) % 2) as f32));
    if !strobe || slow {
        return Err("accessibility: the strobe monitor is wrong");
    }
    let mut fx = CameraFx::new();
    fx.shake(1.0, 1.0);
    fx.kick_fov(0.3, 1.0);
    fx.update(0.05);
    let full = fx.offset(&guarded);
    let damped = fx.offset(&want);
    let (full2, damped2) = (full.dot(full), damped.dot(damped));
    // Squared lengths: the damped shake is at most a fifth of the full one.
    if damped2 > full2 * 0.21 * 0.21 + 1e-8
        || full2 <= 0.0
        || fx.fov_delta(&want) != 0.0
        || fx.fov_delta(&guarded) <= 0.0
    {
        return Err("accessibility: reduced motion does not damp the camera");
    }
    let mut press = InputState::default();
    press.set(trino_core::Button::B, true);
    let idle = InputState::default();
    let (mut hold, mut toggle) = (HoldToggle::new(), HoldToggle::new());
    hold.update(&press, &idle, trino_core::Button::B, &guarded);
    toggle.update(&press, &idle, trino_core::Button::B, &want);
    let held = hold.update(&idle, &press, trino_core::Button::B, &guarded);
    let latched = toggle.update(&idle, &press, trino_core::Button::B, &want);
    if held || !latched {
        return Err("accessibility: hold-to-toggle does not latch");
    }

    // 4. Captions through the audio wrapper.
    let mut captions = Captions::new();
    captions.configure(Some(&data), &want);
    let mut sink = CountingAudio {
        sounds: 0,
        music: 0,
    };
    {
        let mut audio = captions.wrap(&mut sink);
        audio.play_sound(coin);
        audio.play_sound(trino_core::SoundId::from_path("sounds/jump"));
        audio.play_music(trino_core::MusicId::from_path("music/theme"), true);
    }
    if sink.sounds != 2 || sink.music != 1 || captions.len() != 1 {
        return Err("accessibility: the caption wrapper lost a sound or a caption");
    }
    captions.configure(
        Some(&data),
        &AccessSettings {
            captions: CaptionMode::Important,
            ..want
        },
    );
    if captions.on_sound(coin.0, None) || !captions.on_sound(win.0, None) {
        return Err("accessibility: the important mode shows the wrong sounds");
    }

    // 5. The UI at 200 % text, in high contrast, with the icons.
    let bytes = platform
        .blob(UI_SCREEN)
        .ok_or("accessibility: blob ui/selftest missing")?;
    let screen = UiScreen::parse(bytes).map_err(|_| "accessibility: ui/selftest does not parse")?;
    let mut ui: Ui<16> = Ui::new();
    let draw_ui = |r: &mut dyn Renderer, env: &UiEnv<'_>, ui: &mut Ui<16>| {
        for _ in 0..3 {
            r.begin_frame(Color::BLACK);
            ui.draw(r, &screen, env);
            r.end_frame();
        }
        ui.stats()
    };
    let env = UiEnv::of(renderer);
    let plain = draw_ui(renderer, &env, &mut ui);
    let big = draw_ui(
        renderer,
        &env.with_access(&AccessSettings {
            text_scale: 200,
            ..guarded
        }),
        &mut ui,
    );
    if big.overflow != 0 || plain.overflow != 0 {
        return Err("accessibility: a text overflows its button at 200 %");
    }
    let icons = draw_ui(
        renderer,
        &env.with_access(&AccessSettings {
            cvd: CvdMode::Deuteranopia,
            ..guarded
        }),
        &mut ui,
    );
    if icons.draws != plain.draws + 1 {
        return Err("accessibility: the color-vision icon is not drawn");
    }
    let contrast_env = env.with_access(&AccessSettings {
        high_contrast: true,
        ..guarded
    });
    // The first button takes the focus on the first update.
    let _ = ui.update(&screen, &idle, &idle, 0.0);
    draw_ui(renderer, &contrast_env, &mut ui);
    let (_, one) = ui
        .rect_of(&screen, "one")
        .ok_or("accessibility: no `one`")?;
    let probe = (one.pos.x + 4.0, one.pos.y + 3.0);
    let Some(focus) = renderer.read_pixel(probe.0 as u32, probe.1 as u32) else {
        return Ok(false);
    };
    let near = |c: Color, w: Color, t: i32| {
        (c.r as i32 - w.r as i32).abs() <= t
            && (c.g as i32 - w.g as i32).abs() <= t
            && (c.b as i32 - w.b as i32).abs() <= t
    };
    if !near(focus, trino_core::ui::Theme::HIGH_CONTRAST.focus, 40) {
        return Err("accessibility: the high-contrast focus is not its yellow");
    }

    // 6. The palette swap on a slotted card: the same draw, the colors of
    //    the mode.
    let caps = *renderer.caps();
    let size = Vec2::new(caps.screen_width as f32, caps.screen_height as f32);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -1.5),
        target: Vec3::ZERO,
        fov_y: 60.0 * core::f32::consts::PI / 180.0,
    };
    let view = camera.view();
    let half = camera.fov_y * 0.5;
    let focal = cos(half) / sin(half) * size.y * 0.5;
    let names = ["selftest.team_a", "", "", "selftest.team_b"];
    for (mode, want_cols) in [
        (CvdMode::Off, [Color::WHITE, a_base, b_base]),
        (CvdMode::Deuteranopia, [Color::WHITE, a_deut, b_deut]),
    ] {
        let params = ModelParams {
            shading: Shading::Unlit,
            palette: Some(data.palette(names, mode)),
            ..ModelParams::DEFAULT
        };
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            renderer.draw_model(
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &params,
            );
            renderer.end_frame();
        }
        for (x, w) in [-0.4f32, 0.0, 0.4].into_iter().zip(want_cols) {
            let v = view.transform_point(Vec3::new(x, 0.0, 0.0));
            let px = size.x * 0.5 + v.x * focal / v.z;
            let py = size.y * 0.5 - v.y * focal / v.z;
            let got = renderer
                .read_pixel(px as u32, py as u32)
                .ok_or("accessibility: read-back stopped")?;
            if !near(got, w, 48) {
                return Err(if mode == CvdMode::Off {
                    "accessibility: the base palette shows the wrong colors"
                } else {
                    "accessibility: the deuteranopia palette shows the wrong colors"
                });
            }
        }
    }

    // 7. A high-contrast outline: the white card ringed in yellow (a
    //    second, unlit draw pushed behind it), black there without one.
    let ring = trino_core::Outline::new(trino_core::access::OUTLINE_COLOR, Vec3::ZERO, 0.68)
        .with_pixels(6.0);
    let white = ModelParams {
        shading: Shading::Unlit,
        ..ModelParams::DEFAULT
    };
    let edge = view.transform_point(Vec3::new(0.6, 0.0, 0.0));
    let edge_x = size.x * 0.5 + edge.x * focal / edge.z;
    for with in [true, false] {
        for _ in 0..3 {
            renderer.begin_frame(Color::BLACK);
            renderer.set_camera(&camera);
            renderer.set_light(&DEFAULT_LIGHT);
            trino_core::access::draw_outlined(
                renderer,
                &camera,
                SLOTS_CARD,
                &Transform3::default(),
                Material::VertexLit,
                &white,
                with.then_some(&ring),
            );
            renderer.end_frame();
        }
        let out = renderer
            .read_pixel((edge_x + 2.0) as u32, (size.y * 0.5) as u32)
            .ok_or("accessibility: read-back stopped")?;
        let mid = renderer
            .read_pixel((size.x * 0.5) as u32, (size.y * 0.5) as u32)
            .ok_or("accessibility: read-back stopped")?;
        let want_out = if with {
            trino_core::access::OUTLINE_COLOR
        } else {
            Color::BLACK
        };
        if !near(out, want_out, 48) || !near(mid, Color::WHITE, 48) {
            return Err(if with {
                "accessibility: the outline does not ring the card"
            } else {
                "accessibility: without an outline the card is not alone"
            });
        }
    }

    //    At a gameplay distance (a boar-sized card 15 m away), where the
    //    N64's 16-bit z has about 7 cm a step against the copy's 18 cm gap:
    //    the card stays white, the ring still shows.
    let far = Camera3 {
        eye: camera.eye * 10.0,
        ..camera
    };
    let small = Transform3 {
        scale: Vec3::new(0.9, 0.9, 0.9),
        ..Transform3::default()
    };
    let ring = trino_core::Outline::new(trino_core::access::OUTLINE_COLOR, Vec3::ZERO, 0.6)
        .with_pixels(4.0);
    let far_view = far.view();
    let edge = far_view.transform_point(Vec3::new(0.54, 0.0, 0.0));
    let edge_x = size.x * 0.5 + edge.x * focal / edge.z;
    for _ in 0..3 {
        renderer.begin_frame(Color::BLACK);
        renderer.set_camera(&far);
        renderer.set_light(&DEFAULT_LIGHT);
        trino_core::access::draw_outlined(
            renderer,
            &far,
            SLOTS_CARD,
            &small,
            Material::VertexLit,
            &white,
            Some(&ring),
        );
        renderer.end_frame();
    }
    let out = renderer
        .read_pixel((edge_x + 2.0) as u32, (size.y * 0.5) as u32)
        .ok_or("accessibility: read-back stopped")?;
    let mid = renderer
        .read_pixel((size.x * 0.5) as u32, (size.y * 0.5) as u32)
        .ok_or("accessibility: read-back stopped")?;
    if !near(mid, Color::WHITE, 48) {
        return Err("accessibility: a far outline shows through its object");
    }
    if !near(out, trino_core::access::OUTLINE_COLOR, 48) {
        return Err("accessibility: a far outline does not ring its object");
    }

    // 8. The caption box: dark under the text, the scene around it, a
    //    glyph inside; then on the touch screen where there is one.
    let mut captions = Captions::new();
    captions.configure(Some(&data), &want);
    captions.on_sound(coin.0, None);
    let gray = Color::rgb(200, 200, 200);
    for place in [CaptionPlace::Bottom, CaptionPlace::SecondScreen] {
        let style = CaptionStyle {
            font: FONT,
            place,
            ..CaptionStyle::DEFAULT
        };
        let on = match place {
            CaptionPlace::SecondScreen => match renderer.screen_size(Screen::Bottom) {
                Some(_) => Screen::Bottom,
                None => continue,
            },
            _ => Screen::Top,
        };
        for _ in 0..3 {
            renderer.begin_frame(gray);
            if on == Screen::Bottom && renderer.on_screen(Screen::Bottom) {
                // The touch screen's own background, gray like the top's.
                renderer.fill_rect(Rect::new(0.0, 0.0, 320.0, 240.0), gray);
            }
            captions.draw(renderer, &NoStrings, &style);
            renderer.end_frame();
        }
        let screen_px = renderer
            .screen_size(on)
            .ok_or("accessibility: no caption screen")?;
        let boxes = captions.boxes(
            renderer,
            &NoStrings,
            &style,
            screen_px,
            CaptionPlace::Bottom,
        );
        let r = boxes[0].ok_or("accessibility: no caption box")?;
        let read = |r: &mut dyn Renderer, x: f32, y: f32| {
            r.read_screen_pixel(on, x as u32, y as u32)
                .ok_or("accessibility: read-back stopped")
        };
        // The box's left padding, half-way down (clear of the text and of
        // the high-contrast top and bottom rules).
        let inside = read(renderer, r.pos.x + 1.0, r.pos.y + r.size.y * 0.5)?;
        let outside = read(renderer, r.pos.x + 1.0, r.pos.y - 3.0)?;
        if inside.r > 90 || outside.r < 150 {
            return Err("accessibility: the caption box is not where it should be");
        }
        let px = captions.text_px(screen_px.y);
        let y = r.pos.y + 2.0 * px + 3.0 * px;
        let mut glyph = false;
        let mut x = r.pos.x + 2.0 * px;
        while x < r.pos.x + r.size.x - 2.0 * px && !glyph {
            glyph = read(renderer, x, y)?.r > 150;
            x += 1.0;
        }
        if !glyph {
            return Err("accessibility: the caption box has no text");
        }
    }
    Ok(true)
}

/// Rows of [`UI_COST_SCREEN`]'s list.
pub const UI_COST_ROWS: u16 = 41;

/// A screen of 126 laid-out nodes (a 41-row list of a label and a number
/// in a scroll box, issue #95): what the N64 pays to lay out and draw a
/// menu at the UI budget (`TRINO_UI_COST:` in the ares self-test).
pub static UI_COST_SCREEN: trino_core::ui::UiScreen<'static> = {
    use trino_core::ui::{
        Align, Dir, Justify, NONE, NodeKind, Nodes, Scaling, Size, Theme, UiNode, UiScreen,
    };
    static NODES: [UiNode<'static>; 6] = [
        UiNode {
            kind: NodeKind::Overlay,
            end: 6,
            size: (Size::Fill, Size::Fill),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Scroll {
                dir: Dir::Vertical,
                gap: 0,
            },
            parent: 0,
            end: 6,
            size: (Size::Fill, Size::Fill),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::List {
                dir: Dir::Vertical,
                gap: 1,
                max: 64,
            },
            parent: 1,
            end: 6,
            bind: "rows",
            size: (Size::Fill, Size::Fit),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Stack {
                dir: Dir::Horizontal,
                gap: 4,
                align: Align::Center,
                justify: Justify::SpaceBetween,
            },
            parent: 2,
            end: 6,
            size: (Size::Fill, Size::Fit),
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Text,
            parent: 3,
            end: 5,
            text: "{}",
            bind: "name",
            ..UiNode::DEFAULT
        },
        UiNode {
            kind: NodeKind::Text,
            parent: 3,
            end: 6,
            text: "{}",
            bind: "n",
            ..UiNode::DEFAULT
        },
    ];
    UiScreen {
        name: "ui_cost",
        reference: (320, 240),
        scaling: Scaling::Integer,
        wrap: false,
        focus: NONE,
        on_back: "",
        theme: Theme::DEFAULT,
        nodes: Nodes::Slice(&NODES),
    }
};

/// [`UI_COST_SCREEN`]'s values, from the game on demand ([`UiData`]): a
/// new revision lays the screen out again.
///
/// [`UiData`]: trino_core::ui::UiData
pub struct UiCostData {
    pub revision: u32,
}

impl trino_core::ui::UiData for UiCostData {
    fn value(&self, key: &str, item: u16) -> Option<trino_core::ui::UiValue> {
        use trino_core::ui::{UiValue, text_value};
        match key {
            "rows" => Some(UiValue::Int(UI_COST_ROWS as i32)),
            "name" => Some(text_value("PLAYER NAME")),
            "n" => Some(UiValue::Int(item as i32 * 7 + self.revision as i32 % 3)),
            _ => None,
        }
    }

    fn revision(&self) -> Option<u32> {
        Some(self.revision)
    }
}

#[cfg(test)]
mod ui_cost_tests {
    use super::*;

    /// The N64's cost screen sits at the 128-node UI budget and lays out
    /// again on each new revision only.
    #[test]
    fn the_ui_cost_screen_lays_out_126_nodes() {
        use trino_core::ui::{CellMeasure, UiEnv};
        let mut ui = trino_core::Ui::<128>::new();
        let data = UiCostData { revision: 1 };
        let env = UiEnv::of_caps(&trino_core::Caps::N64).with_data(&data);
        ui.layout(&UI_COST_SCREEN, &env, &CellMeasure);
        assert_eq!(ui.stats().nodes, 3 + 3 * UI_COST_ROWS);
        assert!(ui.stats().nodes <= trino_core::Caps::N64.max_ui_nodes);
        assert_eq!(ui.stats().dropped, 0);
    }
}

#[cfg(test)]
mod ui_cost_bench {
    use super::*;
    #[test]
    #[ignore]
    fn bench_layout() {
        use trino_core::ui::{CellMeasure, UiEnv};
        let mut ui = trino_core::Ui::<128>::new();
        let n = 2000;
        let t = std::time::Instant::now();
        for k in 0..n {
            let data = UiCostData { revision: k };
            let env = UiEnv::of_caps(&trino_core::Caps::N64).with_data(&data);
            ui.layout(&UI_COST_SCREEN, &env, &CellMeasure);
        }
        println!(
            "layout {:.1} us",
            t.elapsed().as_secs_f64() * 1e6 / n as f64
        );
    }
}
