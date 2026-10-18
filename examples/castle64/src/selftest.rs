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
    Material, ModelId, ModelParams, Palette, ParticleSystem, Platform, Rect, Renderer, Screen,
    Shading, SpriteId, SpriteParams, TexFilter, TextBuf, TextParams, Transform3, Vec2, Vec3,
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
    platform.log("TRINO_STEP:hw tier\n");
    let tier = hw_tier(renderer, platform)?;
    log_tier(platform, &tier);
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
