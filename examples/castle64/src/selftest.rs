//! Engine feature checks shared by every target's self-test: the PC test
//! suite runs them against the wgpu backend, the N64/3DS apps run them in
//! ares/Azahar (`cargo xtask test n64|3ds`) — same code, same expectations.
//!
//! Each check exercises one engine service through the portable traits
//! only, so a pass on a console proves the backend wiring (shim, asset
//! bake, loader) end to end.

use trino_core::math3d::{cos, sin};
use trino_core::{
    BakedScene, BillboardParams, BlobId, Camera3, Cells, Color, DEFAULT_LIGHT, DetailTexture,
    Effect, Emitter, EntityStore, FontId, Impostor, Light, LocalLight, Material, ModelId,
    ModelParams, ParticleSystem, Platform, Rect, Renderer, Shading, SpriteId, TexFilter, TextBuf,
    TextParams, Transform3, Vec2, Vec3,
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
    platform.log("TRINO_STEP:smooth skin\n");
    smooth_skin(renderer)?;
    platform.log("TRINO_STEP:palette\n");
    if palette(renderer)? {
        platform.log("TRINO_STEP:palette probed 3 slots\n");
    }
    platform.log("TRINO_STEP:local lights\n");
    if local_lights(renderer)? {
        platform.log("TRINO_STEP:local lights probed\n");
    }
    platform.log("TRINO_STEP:impostor\n");
    if impostor(renderer, platform)? {
        platform.log("TRINO_STEP:impostor probed\n");
    }
    platform.log("TRINO_STEP:particles\n");
    particles(renderer, platform)?;
    platform.log("TRINO_STEP:portals\n");
    portals(renderer, platform)?;
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

/// Audio v2 through the real backend: a steerable looping voice (start,
/// steer, stop) and a stinger over the music.
pub fn audio_checks(audio: &mut dyn trino_core::Audio) -> Result<(), &'static str> {
    use trino_core::SoundParams;
    let left = SoundParams {
        volume: 0.5,
        pan: -1.0,
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
        },
    );
    Ok(())
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
