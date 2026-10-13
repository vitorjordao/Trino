//! Engine feature checks shared by every target's self-test: the PC test
//! suite runs them against the wgpu backend, the N64/3DS apps run them in
//! ares/Azahar (`cargo xtask test n64|3ds`) — same code, same expectations.
//!
//! Each check exercises one engine service through the portable traits
//! only, so a pass on a console proves the backend wiring (shim, asset
//! bake, loader) end to end.

use trino_core::math3d::{cos, sin};
use trino_core::{
    BillboardParams, BlobId, Camera3, Color, DEFAULT_LIGHT, Emitter, FontId, Material, ModelId,
    ModelParams, ParticleSystem, Platform, Rect, Renderer, Shading, SpriteId, TextParams,
    Transform3, Vec2, Vec3,
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
    platform.log("TRINO_STEP:billboards\n");
    if billboards(renderer)? {
        platform.log("TRINO_STEP:billboards probed\n");
    }
    platform.log("TRINO_STEP:engine ok\n");
    Ok(())
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
