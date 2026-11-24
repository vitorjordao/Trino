//! Mipmap chains (Bongo-Kart #178) on the PC renderer: a floor tiled with
//! a one-texel checker, seen at a grazing angle, shimmers in moiré without
//! a chain and settles to the checker's mean light with one; the chain is
//! made in linear light (the far floor is sRGB 188, not 128); the PC's
//! console profiles and every 2D draw sample level 0, so they draw what
//! they drew before; the BC chain a bake writes draws like the RGBA8 one
//! made at upload. Evidence PNGs in `target/evidence/mipmaps`. GPU-gated
//! like the goldens.

use std::path::{Path, PathBuf};

use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    Color, DynVertex, DynamicMesh, Mat34, Material, ModelId, ModelParams, Renderer, Shading,
    SpriteId, SpriteParams, TexFilter, Vec2, Vec3,
};
use trino_platform_pc::{CompressedFormat, PcRenderer, SimProfile};

const FLOOR: ModelId = ModelId(31);
/// The checker uploaded with its chain.
const MIPPED: SpriteId = SpriteId(41);
/// The same pixels, level 0 alone.
const PLAIN: SpriteId = SpriteId(42);
/// The same chain as BC1 blocks (a bake's `TBCN` levels).
const BC_MIPPED: SpriteId = SpriteId(43);
const SIDE: u32 = 32;
const CLEAR: Color = Color::rgb(40, 60, 90);

/// One GPU test at a time (parallel headless devices crash some drivers).
fn gpu() -> std::sync::MutexGuard<'static, ()> {
    static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(mut r) => {
            if profile == SimProfile::Native {
                r.set_internal_size(480, 270);
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

fn evidence(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../target/evidence/mipmaps")
        .join(name)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let mut e = png::Encoder::new(std::fs::File::create(path).unwrap(), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// A one-texel black and white checker: the worst case for minification.
fn checker() -> Vec<u8> {
    let mut px = Vec::new();
    for y in 0..SIDE {
        for x in 0..SIDE {
            let v = if (x + y) % 2 == 0 { 255 } else { 0 };
            px.extend_from_slice(&[v, v, v, 255]);
        }
    }
    px
}

/// A 40 × 120 m floor, its texture repeating every 2 m (a 32-texel tile).
fn floor_blob() -> Vec<u8> {
    let n = Vec3::new(0.0, 1.0, 0.0);
    let white = Color::rgb(255, 255, 255);
    let corner = |x: f32, z: f32| DynVertex {
        uv: Vec2::new(x / 2.0, z / 2.0),
        ..DynVertex::new(Vec3::new(x, 0.0, z), n, white)
    };
    let v = [
        corner(-20.0, 0.0),
        corner(20.0, 0.0),
        corner(20.0, 120.0),
        corner(-20.0, 120.0),
    ];
    let i = [0u16, 2, 1, 0, 3, 2];
    let mesh = DynamicMesh::new(&v, &i).textured(MIPPED);
    let mut b = vec![0; mesh.tmdl_len().unwrap()];
    assert_eq!(mesh.write_tmdl(&mut b), Ok(b.len()));
    b
}

fn upload(r: &mut PcRenderer) {
    r.upload_mesh(FLOOR, floor_blob());
    r.upload_sprite_mipmapped(MIPPED, SIDE, SIDE, &checker());
    r.upload_sprite(PLAIN, SIDE, SIDE, &checker());
}

/// The floor from 1.2 m up, looking at the horizon.
fn floor_frame(r: &mut PcRenderer, texture: SpriteId) -> Vec<u8> {
    r.begin_frame(CLEAR);
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 1.2, -2.0),
        target: Vec3::new(0.0, 0.7, 10.0),
        fov_y: 60f32.to_radians(),
    });
    r.set_light(&DEFAULT_LIGHT);
    let params = ModelParams {
        shading: Shading::Unlit,
        texture: Some(texture),
        filter: TexFilter::Bilinear,
        ..ModelParams::DEFAULT
    };
    r.draw_model_matrix(FLOOR, &Mat34::IDENTITY, Material::VertexLit, &params);
    r.end_frame();
    r.read_offscreen()
}

/// The far floor: its first `rows` rows below the horizon that are floor
/// from edge to edge. Their mean luma and the mean luma step between
/// horizontal neighbors (the moiré).
fn far_band(px: &[u8], w: usize, rows: usize) -> (f32, f32) {
    let luma = |i: usize| {
        let p = &px[i * 4..i * 4 + 3];
        (p[0] as f32 * 0.2126 + p[1] as f32 * 0.7152 + p[2] as f32 * 0.0722).round()
    };
    let h = px.len() / 4 / w;
    let is_clear = |i: usize| px[i * 4..i * 4 + 3] == [CLEAR.r, CLEAR.g, CLEAR.b];
    let floor_rows: Vec<usize> = (0..h)
        .filter(|&y| (0..w).all(|x| !is_clear(y * w + x)))
        .take(rows)
        .collect();
    assert_eq!(floor_rows.len(), rows, "the floor fills {rows} rows");
    let (mut sum, mut step, mut n) = (0.0, 0.0, 0.0);
    for y in floor_rows {
        for x in 0..w - 1 {
            let i = y * w + x;
            sum += luma(i);
            step += (luma(i) - luma(i + 1)).abs();
            n += 1.0;
        }
    }
    (sum / n, step / n)
}

#[test]
fn a_chain_turns_far_moire_into_the_checkers_mean_light() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    upload(&mut r);
    assert_eq!(r.sprite_levels(MIPPED), 4, "32, 16, 8, 4");
    assert_eq!(r.sprite_levels(PLAIN), 1);
    assert_eq!(
        r.sprite_bytes(MIPPED),
        trino_core::mip::rgba8_chain_bytes(SIDE, SIDE) as u64,
        "the chain's memory: a third more than level 0"
    );
    assert!(r.samples_mipmaps());
    let (w, h) = r.internal_size();
    let plain = floor_frame(&mut r, PLAIN);
    let on = floor_frame(&mut r, MIPPED);
    r.set_mipmaps(false);
    let off = floor_frame(&mut r, MIPPED);
    r.set_mipmaps(true);
    assert!(off == plain, "mipmaps off: the chain is not sampled");
    write_png(&evidence("native_off.png"), w, h, &off);
    write_png(&evidence("native_on.png"), w, h, &on);
    let (mean_off, moire_off) = far_band(&off, w as usize, 40);
    let (mean_on, moire_on) = far_band(&on, w as usize, 40);
    eprintln!(
        "far floor (40 rows): without a chain mean {mean_off:.1} step {moire_off:.1}; \
         with it mean {mean_on:.1} step {moire_on:.1}"
    );
    assert!(
        moire_on * 4.0 < moire_off,
        "the chain must calm the moiré: {moire_on:.1} vs {moire_off:.1}"
    );
    // Half the light of black and white is sRGB 188; averaging the codes
    // would give 128 (a far floor darker than the near one).
    assert!(
        (mean_on - 188.0).abs() < 16.0,
        "the far floor is the checker's mean light: {mean_on:.1}"
    );
}

#[test]
fn console_profiles_and_2d_draws_sample_level_0() {
    let _g = gpu();
    for profile in [SimProfile::N64, SimProfile::N3ds] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        upload(&mut r);
        assert!(!r.samples_mipmaps(), "{profile:?}: Caps::mipmaps is off");
        let plain = floor_frame(&mut r, PLAIN);
        let mipped = floor_frame(&mut r, MIPPED);
        assert!(mipped == plain, "{profile:?}: the console's frame changed");
        if profile == SimProfile::N64 {
            let (w, h) = r.internal_size();
            write_png(&evidence("n64_profile.png"), w, h, &mipped);
        }
    }
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    upload(&mut r);
    // A 2D draw minified to a quarter: level 0 either way (the HUD keeps
    // its pixels).
    let sprite = |r: &mut PcRenderer, id: SpriteId| {
        r.begin_frame(CLEAR);
        let params = SpriteParams {
            scale: Vec2::new(0.25, 0.25),
            ..SpriteParams::DEFAULT
        };
        r.draw_sprite(id, Vec2::new(20.0, 20.0), &params);
        r.end_frame();
        r.read_offscreen()
    };
    assert!(
        sprite(&mut r, MIPPED) == sprite(&mut r, PLAIN),
        "2D: level 0"
    );
}

#[test]
fn a_bakes_bc_chain_draws_like_the_chain_made_at_upload() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    upload(&mut r);
    if !r.supports_compressed(CompressedFormat::Bc1) {
        eprintln!("skipping: no BC on this GPU");
        return;
    }
    let bc = trino_asset_pipeline::BcFormat::Bc1;
    let blocks = bc.encode_chain(SIDE, SIDE, &checker());
    let levels = bc.levels(SIDE, SIDE, &blocks).expect("a whole chain");
    assert_eq!(levels.len(), 4);
    // One level short, or a level of the wrong size: refused.
    assert!(!r.upload_sprite_compressed_levels(
        BC_MIPPED,
        SIDE,
        SIDE,
        CompressedFormat::Bc1,
        &levels[..3],
        None
    ));
    let mut short = levels.clone();
    short[2] = &levels[2][..8];
    assert!(!r.upload_sprite_compressed_levels(
        BC_MIPPED,
        SIDE,
        SIDE,
        CompressedFormat::Bc1,
        &short,
        None
    ));
    assert!(r.upload_sprite_compressed_levels(
        BC_MIPPED,
        SIDE,
        SIDE,
        CompressedFormat::Bc1,
        &levels,
        None
    ));
    assert_eq!(r.sprite_levels(BC_MIPPED), 4);
    let rgba = floor_frame(&mut r, MIPPED);
    let blocks = floor_frame(&mut r, BC_MIPPED);
    let diff: u64 = rgba
        .iter()
        .zip(&blocks)
        .map(|(a, b)| (*a as i32 - *b as i32).unsigned_abs() as u64)
        .sum();
    let mean = diff as f64 / rgba.len() as f64;
    assert!(mean < 2.0, "BC1 chain vs RGBA8 chain: mean diff {mean:.2}");
}

#[test]
fn sizes_that_cant_carry_a_chain_upload_level_0() {
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    let odd = vec![200u8; 24 * 24 * 4];
    r.upload_sprite_mipmapped(PLAIN, 24, 24, &odd);
    assert_eq!(r.sprite_levels(PLAIN), 1, "24 is not a power of two");
    assert_eq!(r.sprite_bytes(PLAIN), 24 * 24 * 4);
}

/// The issue's image comparison per quality preset: the far floor with and
/// without the chain at Low, Medium, High and Ultra (render scale,
/// anisotropy and MSAA change, the moiré doesn't go away without mips).
/// Evidence: `target/evidence/mipmaps/preset_<name>_{on,off}.png`.
#[test]
fn every_quality_preset_gains_from_the_chain() {
    use trino_core::{QualityPreset, QualitySettings};
    let _g = gpu();
    let Some(mut r) = headless(SimProfile::Native) else {
        return;
    };
    upload(&mut r);
    let (w, h) = r.internal_size();
    for preset in [
        QualityPreset::Low,
        QualityPreset::Medium,
        QualityPreset::High,
        QualityPreset::Ultra,
    ] {
        r.apply_quality(&QualitySettings {
            dynamic_resolution: None,
            ..QualitySettings::preset(preset)
        });
        r.set_mipmaps(false);
        let off = floor_frame(&mut r, MIPPED);
        r.set_mipmaps(true);
        let on = floor_frame(&mut r, MIPPED);
        let name = format!("{preset:?}").to_lowercase();
        write_png(&evidence(&format!("preset_{name}_off.png")), w, h, &off);
        write_png(&evidence(&format!("preset_{name}_on.png")), w, h, &on);
        let (mean_off, moire_off) = far_band(&off, w as usize, 40);
        let (mean_on, moire_on) = far_band(&on, w as usize, 40);
        eprintln!(
            "{name}: without a chain mean {mean_off:.1} step {moire_off:.1}; \
             with it mean {mean_on:.1} step {moire_on:.1}"
        );
        // The anisotropic presets (Medium 4×, High 8×, Ultra 16×) sample a
        // sharper level along the view: a one-texel checker sits right at
        // the Nyquist limit there and keeps some moiré, less than without
        // the chain (Ultra's 16× about as much). Low (no anisotropy) loses
        // it all. Never worse with the chain.
        assert!(
            moire_on <= moire_off * 1.05,
            "{name}: {moire_on:.1} vs {moire_off:.1}"
        );
        assert!(
            (mean_on - 188.0).abs() <= (mean_off - 188.0).abs() + 1.0,
            "{name}: the far floor's light {mean_on:.1} vs {mean_off:.1} (188 is right)"
        );
        if preset == QualityPreset::Low {
            assert!(moire_on < 2.0 && (mean_on - 188.0).abs() < 8.0, "low");
        }
    }
}
