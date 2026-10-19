//! Accessibility on the PC renderer (issue #60):
//!
//! - the palette swap golden: castle64's door colors from the baked
//!   `assets/palettes.ron` on a slotted card, one card per color-vision
//!   mode, the same draw with the mode's palette, and a caption box, on the
//!   N64 profile (`tests/golden/pc/access_palettes.png`);
//! - the same frame through the deuteranopia simulation view
//!   (`access_cvd_deuteranopia.png`): the alternatives stay apart where the
//!   base colors would merge;
//! - the simulation view against the core's CPU simulation, color by color.
//!
//! GPU-gated like the other goldens (`TRINO_REQUIRE_GPU=1` turns a missing
//! adapter into a failure); `TRINO_BLESS=1` rewrites them.

use std::path::{Path, PathBuf};

use trino_core::access::color::simulate;
use trino_core::render3d::Camera3;
use trino_core::{
    AccessData, AccessSettings, CaptionMode, CaptionStyle, Captions, Color, CvdMode, FontId,
    Material, ModelId, ModelParams, Platform, Rect, Renderer, Shading, TextAlign, TextParams,
    Transform3, Vec2, Vec3, asset_id,
};
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

const SLOTS_CARD: ModelId = ModelId::from_path("models/selftest_slots");
const FONT: FontId = FontId::from_path("fonts/trino5x7");
/// Pixels between two cards' centers (1.45 units at 7 units, 50° FOV).
const CARD_PX: f32 = 53.3;

fn repo() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// A renderer with the engine's baked assets and a platform with its
/// blobs; `None` (skip) without a GPU.
fn target(profile: SimProfile) -> Option<(PcRenderer, PcPlatform)> {
    let root = repo();
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    let mut p = PcPlatform::new(&profile.caps());
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for f in assets.fonts {
        r.upload_font(FontId(f.id), f.table);
    }
    for m in assets.models {
        r.upload_mesh(ModelId(m.id), m.tmdl);
    }
    for b in assets.blobs {
        p.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    Some((r, p))
}

/// Five cards, one per mode, each in that mode's door colors (slot 1 the
/// red door, slot 4 the green one), the mode under it, and a caption.
fn palettes_frame(
    r: &mut PcRenderer,
    data: &AccessData<'static>,
    strings: &dyn trino_core::ui::UiStrings,
) {
    let size = Vec2::new(320.0, 240.0);
    let camera = Camera3 {
        eye: Vec3::new(0.0, 0.0, -7.0),
        target: Vec3::ZERO,
        fov_y: 50.0 * core::f32::consts::PI / 180.0,
    };
    let mut captions = Captions::new();
    captions.configure(
        Some(data),
        &AccessSettings {
            captions: CaptionMode::All,
            ..AccessSettings::DEFAULT
        },
    );
    captions.on_sound(asset_id("sounds/win"), None);
    let labels = ["OFF", "PROT", "DEUT", "TRIT", "GRAY"];
    for _ in 0..2 {
        r.begin_frame(Color::rgb(24, 26, 40));
        r.set_camera(&camera);
        r.set_light(&trino_core::DEFAULT_LIGHT);
        for (k, mode) in CvdMode::ALL.into_iter().enumerate() {
            let x = (k as f32 - 2.0) * 1.45;
            let palette = data.palette(["castle64.door.red", "", "", "castle64.door.green"], mode);
            r.draw_model(
                SLOTS_CARD,
                &Transform3 {
                    position: Vec3::new(x, 0.3, 0.0),
                    ..Transform3::default()
                },
                Material::VertexLit,
                &ModelParams {
                    shading: Shading::Unlit,
                    palette: Some(palette),
                    ..ModelParams::DEFAULT
                },
            );
            r.draw_text(
                FONT,
                Vec2::new(size.x * 0.5 + (k as f32 - 2.0) * CARD_PX, 80.0),
                labels[k],
                &TextParams {
                    scale: 1.0,
                    tint: Color::WHITE,
                    align: TextAlign::Center,
                },
            );
        }
        captions.draw(
            r,
            strings,
            &CaptionStyle {
                font: FONT,
                ..CaptionStyle::DEFAULT
            },
        );
        r.end_frame();
    }
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let f = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(f), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// One RGBA5551 step and the dither ties drivers round differently, as in
/// the UI goldens on the N64 profile.
fn check(name: &str, w: u32, h: u32, rgba: &[u8], failures: &mut Vec<String>) {
    let golden = repo().join("tests/golden/pc");
    let path = golden.join(format!("{name}.png"));
    if std::env::var("TRINO_BLESS").is_ok() {
        write_png(&path, w, h, rgba);
        eprintln!("blessed golden: {}", path.display());
        return;
    }
    assert!(
        path.exists(),
        "golden missing at {} — run `cargo xtask test --bless` once and commit it",
        path.display()
    );
    let (gw, gh, want) = read_png(&path);
    assert_eq!((gw, gh), (w, h), "{name}: size");
    let bad = rgba
        .iter()
        .zip(&want)
        .filter(|(a, b)| a.abs_diff(**b) > 9)
        .count();
    if bad > 256 {
        let actual = golden.join(format!("actual/{name}.png"));
        write_png(&actual, w, h, rgba);
        failures.push(format!(
            "{name}: {bad} channel values differ by more than 9 (actual at {})",
            actual.display()
        ));
    }
}

#[test]
fn palette_swap_and_simulation_match_their_goldens() {
    let Some((mut r, mut p)) = target(SimProfile::N64) else {
        return;
    };
    let mut blob = trino_core::AccessBlob::new();
    let data = *blob
        .get(&mut p as &mut dyn Platform)
        .expect("access/data baked");
    let mut en = trino_core::ui::StringsBlob::new("en-US");
    let strings = *en.get(&p).expect("i18n/en-US baked");
    let mut failures = Vec::new();
    palettes_frame(&mut r, &data, &strings);
    let (w, h) = r.internal_size();
    let plain = r.read_offscreen();
    check("access_palettes", w, h, &plain, &mut failures);
    // The deuteranopia card (the third) shows its alternatives: the swap
    // happened in the same draw.
    let px = |rgba: &[u8], x: u32, y: u32| {
        let i = ((y * w + x) * 4) as usize;
        Color::rgb(rgba[i], rgba[i + 1], rgba[i + 2])
    };
    let red_off = px(&plain, (160.0 - CARD_PX * 2.0) as u32, 109);
    let red_deut = px(&plain, 160, 109);
    assert!(red_off.r > 150 && red_off.g < 130, "{red_off:?}");
    assert!(red_deut.r < 160 && red_deut.b < 60, "{red_deut:?}");
    r.set_cvd_view(CvdMode::Deuteranopia);
    palettes_frame(&mut r, &data, &strings);
    let seen = r.read_offscreen();
    check("access_cvd_deuteranopia", w, h, &seen, &mut failures);
    assert!(failures.is_empty(), "{failures:#?}");
}

/// The simulation view (a GPU pass) against `trino_core::access::color::
/// simulate` (the CPU, table-driven): every mode, a dozen colors, within
/// 2/255 (the shader's `pow` against the tables' rounding).
#[test]
fn the_simulation_view_matches_the_cpu_simulation() {
    let Some((mut r, _)) = target(SimProfile::Native) else {
        return;
    };
    r.set_internal_size(240, 40);
    let colors = [
        Color::rgb(220, 40, 40),
        Color::rgb(40, 160, 60),
        Color::rgb(40, 90, 220),
        Color::rgb(240, 160, 0),
        Color::rgb(255, 255, 255),
        Color::rgb(0, 0, 0),
        Color::rgb(128, 128, 128),
        Color::rgb(255, 0, 255),
        Color::rgb(0, 255, 255),
        Color::rgb(255, 255, 0),
        Color::rgb(95, 205, 95),
        Color::rgb(230, 95, 80),
    ];
    for mode in CvdMode::ALL {
        r.set_cvd_view(mode);
        r.begin_frame(Color::BLACK);
        for (k, c) in colors.iter().enumerate() {
            r.fill_rect(Rect::new(k as f32 * 20.0, 0.0, 20.0, 40.0), *c);
        }
        r.end_frame();
        let rgba = r.read_offscreen();
        for (k, c) in colors.iter().enumerate() {
            let i = (20 * 240 + k * 20 + 10) * 4;
            let got = Color::rgb(rgba[i], rgba[i + 1], rgba[i + 2]);
            let want = simulate(mode, *c);
            let near = |a: u8, b: u8| a.abs_diff(b) <= 2;
            assert!(
                near(got.r, want.r) && near(got.g, want.g) && near(got.b, want.b),
                "{mode:?} {c:?}: GPU {got:?}, CPU {want:?}"
            );
        }
    }
    assert_eq!(r.cvd_view(), CvdMode::Grayscale);
}

/// Strict mode warns on a raw strobe drawn with `fill_rect` (more than
/// three full-screen flashes a second) and never on flashes drawn through
/// `ScreenFlash` under the guard.
#[test]
fn strict_mode_finds_a_raw_strobe() {
    let Some((mut r, _)) = target(SimProfile::N64) else {
        return;
    };
    r.set_strict(true);
    let guard = AccessSettings::DEFAULT;
    let mut flash = trino_core::ScreenFlash::new();
    for f in 0..120 {
        if f % 5 == 0 {
            flash.trigger(Color::WHITE, 1.0 / 30.0, &guard);
        }
        r.begin_frame(Color::BLACK);
        flash.draw(&mut r);
        r.end_frame();
        flash.update(1.0 / 60.0);
    }
    assert_eq!(r.flash_strobes(), 0, "the guarded flash");
    // A calm second between the two.
    for _ in 0..90 {
        r.begin_frame(Color::BLACK);
        r.end_frame();
    }
    for f in 0..120 {
        r.begin_frame(Color::BLACK);
        if (f / 5) % 2 == 0 {
            r.fill_rect(Rect::new(0.0, 0.0, 320.0, 240.0), Color::WHITE);
        }
        r.end_frame();
    }
    assert_eq!(r.flash_strobes(), 1, "a 6 Hz white strobe");
}
