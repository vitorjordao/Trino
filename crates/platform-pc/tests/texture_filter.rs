//! Texture filtering per draw (`ModelParams::filter`, `set_texture_filter`)
//! and the detail layer (`ModelParams::detail`) on the PC renderer, in each
//! profile's look: properties that hold on any GPU, plus one golden per
//! profile (`tests/golden/pc/texture_filter_*`, `cargo xtask test --bless`)
//! with three cards side by side — point, bilinear, bilinear + detail.

use std::path::PathBuf;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{
    Color, DetailTexture, Material, ModelId, ModelParams, Renderer, Shading, SpriteId, TexFilter,
    Transform3, Vec3,
};
use trino_platform_pc::{PcRenderer, SimProfile};

const CARD: ModelId = ModelId(0x2001);
const QUADS: SpriteId = SpriteId(0x2002);
const GRAIN: SpriteId = SpriteId(0x2003);
const BIG: SpriteId = SpriteId(0x2004);

/// One GPU test at a time in this binary: RADV crashed (SIGSEGV, ~1 run in
/// 10–40) creating and dropping several devices from parallel test threads
/// — `render_e2e` too, before these tests existed.
static GPU: std::sync::Mutex<()> = std::sync::Mutex::new(());

fn gpu() -> std::sync::MutexGuard<'static, ()> {
    GPU.lock().unwrap_or_else(|e| e.into_inner())
}

fn headless(profile: SimProfile) -> Option<PcRenderer> {
    match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => Some(r),
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but no adapter: {e}");
            }
            eprintln!("skipping: no GPU adapter ({e})");
            None
        }
    }
}

/// A 1×1 card facing -Z with planar UVs (u = x + 0.5, v = 0.5 - y).
fn card_tmdl() -> Vec<u8> {
    let pos = [(0.5f32, -0.5f32), (-0.5, -0.5), (-0.5, 0.5), (0.5, 0.5)];
    let mut b = Vec::new();
    b.extend_from_slice(b"TMDL");
    b.extend_from_slice(&4u32.to_le_bytes());
    b.extend_from_slice(&6u32.to_le_bytes());
    for (x, y) in pos {
        for v in [x, y, 0.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    for _ in 0..4 {
        for v in [0.0f32, 0.0, -1.0] {
            b.extend_from_slice(&v.to_le_bytes());
        }
    }
    for _ in 0..4 {
        b.extend_from_slice(&[255, 255, 255, 255]);
    }
    for i in [0u16, 1, 2, 0, 2, 3] {
        b.extend_from_slice(&i.to_le_bytes());
    }
    let mut uv = Vec::new();
    for (x, y) in pos {
        uv.extend_from_slice(&(x + 0.5).to_le_bytes());
        uv.extend_from_slice(&(0.5 - y).to_le_bytes());
    }
    b.extend_from_slice(b"TXUV");
    b.extend_from_slice(&(uv.len() as u32).to_le_bytes());
    b.extend_from_slice(&uv);
    b
}

/// 16×16: red, green / blue, white quadrants.
fn quadrants() -> Vec<u8> {
    let mut t = Vec::new();
    for y in 0..16 {
        for x in 0..16 {
            t.extend_from_slice(match (x < 8, y < 8) {
                (true, true) => &[255, 0, 0, 255],
                (false, true) => &[0, 255, 0, 255],
                (true, false) => &[0, 0, 255, 255],
                (false, false) => &[255, 255, 255, 255],
            });
        }
    }
    t
}

/// 8×8 grain: white left half, gray right half.
fn grain() -> Vec<u8> {
    let mut t = Vec::new();
    for _ in 0..8 {
        for x in 0..8 {
            let v = if x < 4 { 255 } else { 128 };
            t.extend_from_slice(&[v, v, v, 255]);
        }
    }
    t
}

fn setup(r: &mut PcRenderer) {
    r.upload_mesh(CARD, card_tmdl());
    r.upload_sprite(QUADS, 16, 16, &quadrants());
    r.upload_sprite(GRAIN, 8, 8, &grain());
}

const CAMERA: Camera3 = Camera3 {
    eye: Vec3::new(0.0, 0.0, -1.2),
    target: Vec3::ZERO,
    fov_y: 60.0 * core::f32::consts::PI / 180.0,
};

fn base() -> ModelParams {
    ModelParams {
        shading: Shading::Unlit,
        texture: Some(QUADS),
        ..ModelParams::DEFAULT
    }
}

/// One card at `x` per entry.
fn frame(r: &mut PcRenderer, cards: &[(f32, ModelParams)], camera: &Camera3) -> Vec<u8> {
    r.begin_frame(Color::rgb(20, 20, 30));
    r.set_camera(camera);
    r.set_light(&DEFAULT_LIGHT);
    for (x, p) in cards {
        r.draw_model(
            CARD,
            &Transform3 {
                position: Vec3::new(*x, 0.0, 0.0),
                ..Default::default()
            },
            Material::VertexLit,
            p,
        );
    }
    r.end_frame();
    r.read_offscreen()
}

/// The pixel showing card-space `(u, v)` of the card at the origin.
fn at(r: &PcRenderer, img: &[u8], u: f32, v: f32) -> [u8; 3] {
    let (w, h) = r.internal_size();
    let p = CAMERA
        .view()
        .transform_point(Vec3::new(u - 0.5, 0.5 - v, 0.0));
    let half = CAMERA.fov_y * 0.5;
    let focal = half.cos() / half.sin() * h as f32 * 0.5;
    let x = (w as f32 * 0.5 + p.x * focal / p.z) as u32;
    let y = (h as f32 * 0.5 - p.y * focal / p.z) as u32;
    let i = ((y * w + x) * 4) as usize;
    [img[i], img[i + 1], img[i + 2]]
}

const PROFILES: [SimProfile; 3] = [SimProfile::Native, SimProfile::N3ds, SimProfile::N64];

#[test]
fn point_keeps_texels_and_bilinear_blends_them_in_every_look() {
    let _gpu = gpu();
    for profile in PROFILES {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        for (filter, blended) in [(TexFilter::Point, false), (TexFilter::Bilinear, true)] {
            let img = frame(&mut r, &[(0.0, ModelParams { filter, ..base() })], &CAMERA);
            // 0.24 texel left of the red/green edge.
            let c = at(&r, &img, 0.485, 0.25);
            assert!(c[0] > 150, "{profile:?} {filter:?}: {c:?}");
            if blended {
                assert!(c[1] > 40, "{profile:?} Bilinear must blend: {c:?}");
            } else {
                assert!(c[1] <= 30, "{profile:?} Point must not blend: {c:?}");
            }
        }
    }
}

#[test]
fn the_default_filter_is_the_profiles_and_set_texture_filter_changes_it() {
    let _gpu = gpu();
    for (profile, blended) in [
        (SimProfile::Native, false),
        (SimProfile::N3ds, true),
        (SimProfile::N64, true),
    ] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let g = |r: &mut PcRenderer| {
            let img = frame(r, &[(0.0, base())], &CAMERA);
            at(r, &img, 0.485, 0.25)[1]
        };
        assert_eq!(g(&mut r) > 40, blended, "{profile:?} default");
        r.set_texture_filter(if blended {
            TexFilter::Point
        } else {
            TexFilter::Bilinear
        });
        assert_eq!(g(&mut r) > 40, !blended, "{profile:?} switched");
        r.set_texture_filter(TexFilter::Default);
        assert_eq!(g(&mut r) > 40, blended, "{profile:?} back to default");
    }
}

#[test]
fn the_detail_layer_multiplies_the_texture_on_every_path() {
    let _gpu = gpu();
    for profile in PROFILES {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        // Unlit, and (native) lit per pixel with gloss.
        let shadings: &[ModelParams] = if profile == SimProfile::Native {
            &[
                ModelParams {
                    shading: Shading::Unlit,
                    ..ModelParams::DEFAULT
                },
                ModelParams {
                    shading: Shading::Unlit,
                    gloss: 50,
                    ..ModelParams::DEFAULT
                },
            ]
        } else {
            &[ModelParams::DEFAULT]
        };
        for s in shadings {
            let p = ModelParams {
                texture: Some(QUADS),
                filter: TexFilter::Point,
                detail: Some(DetailTexture::new(GRAIN, 0)),
                shading: Shading::Unlit,
                gloss: s.gloss,
                ..ModelParams::DEFAULT
            };
            let img = frame(&mut r, &[(0.0, p)], &CAMERA);
            let near = |a: u8, b: u8| a.abs_diff(b) <= 24;
            let tl = at(&r, &img, 0.25, 0.25);
            let tr = at(&r, &img, 0.75, 0.25);
            assert!(
                near(tl[0], 255) && tl[1] < 24,
                "{profile:?} white detail: {tl:?}"
            );
            assert!(
                near(tr[1], 128) && tr[0] < 24,
                "{profile:?} gray detail: {tr:?}"
            );
            // Twice as fine: the left quadrant now has a gray strip too.
            let p2 = ModelParams {
                detail: Some(DetailTexture::new(GRAIN, 1)),
                ..p
            };
            let img = frame(&mut r, &[(0.0, p2)], &CAMERA);
            let strip = at(&r, &img, 0.375, 0.25);
            assert!(near(strip[0], 128), "{profile:?} scale 1: {strip:?}");
        }
    }
}

#[test]
fn strict_mode_rejects_a_detail_pair_over_the_n64_tmem() {
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    setup(&mut r);
    r.set_strict(true);
    // 32×32 + 8×8 at 16 bits: 2.2 KB, fits.
    r.upload_sprite(BIG, 32, 32, &vec![200; 32 * 32 * 4]);
    let detail = Some(DetailTexture::new(GRAIN, 2));
    let fits = ModelParams {
        texture: Some(BIG),
        detail,
        ..base()
    };
    frame(&mut r, &[(0.0, fits)], &CAMERA);
    // 32×64 + 8×8: 4.2 KB, over 4096.
    r.upload_sprite(BIG, 32, 64, &vec![200; 32 * 64 * 4]);
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        frame(&mut r, &[(0.0, fits)], &CAMERA);
    }))
    .expect_err("strict mode must reject base + detail over TMEM");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("detail") && msg.contains("TMEM"), "{msg}");
}

/// GitHub #125: the base and detail count in the formats the N64 bake
/// chose. A 64×64 I4 base (2 KB) with an 8×8 detail fits, where 16-bit
/// texels (8 KB) would not; a palette base can't take a detail at all (its
/// palette fills TMEM's upper half and the console draws it without one).
#[test]
fn strict_mode_counts_the_detail_pair_in_their_n64_formats() {
    use trino_core::TexelFormat;
    let _gpu = gpu();
    let Some(mut r) = headless(SimProfile::N64) else {
        return;
    };
    setup(&mut r);
    r.set_strict(true);
    r.upload_sprite(BIG, 64, 64, &vec![200; 64 * 64 * 4]);
    r.set_texture_format(BIG, Some(TexelFormat::I4));
    r.set_texture_format(GRAIN, Some(TexelFormat::I4));
    let p = ModelParams {
        texture: Some(BIG),
        detail: Some(DetailTexture::new(GRAIN, 2)),
        ..base()
    };
    frame(&mut r, &[(0.0, p)], &CAMERA);
    // The same pair without formats is 16-bit texels: 8.3 KB.
    r.set_texture_format(BIG, None);
    let unknown = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        frame(&mut r, &[(0.0, p)], &CAMERA);
    }));
    assert!(unknown.is_err(), "64×64 at 16 bits + detail is over 4 KB");
    // A CI4 base the same size as the I4 one: rejected for its palette.
    r.set_texture_format(BIG, Some(TexelFormat::Ci4));
    let err = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        frame(&mut r, &[(0.0, p)], &CAMERA);
    }))
    .expect_err("a palette base must not take a detail layer");
    let msg = err.downcast_ref::<String>().cloned().unwrap_or_default();
    assert!(msg.contains("palette") && msg.contains("Ci4"), "{msg}");
    // Formats are the N64's: the 3DS profile keeps its own rule.
    let Some(mut r3) = headless(SimProfile::N3ds) else {
        return;
    };
    setup(&mut r3);
    r3.set_strict(true);
    r3.upload_sprite(BIG, 64, 64, &vec![200; 64 * 64 * 4]);
    r3.set_texture_format(BIG, Some(TexelFormat::Ci4));
    frame(&mut r3, &[(0.0, p)], &CAMERA);
}

fn write_png(path: &std::path::Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut e = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    e.set_color(png::ColorType::Rgba);
    e.set_depth(png::BitDepth::Eight);
    e.write_header().unwrap().write_image_data(rgba).unwrap();
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let d = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = d.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

/// The frame's GPU T&L draws against the CPU path (`tnl_oracle`, issue #51),
/// the side-by-side kept under `target/evidence/pc-gpu-tnl`. Returns the
/// GPU draws (0 on a console profile).
fn tnl_oracle(
    r: &mut PcRenderer,
    name: &str,
    frame: impl FnMut(&mut PcRenderer) -> Vec<u8>,
) -> u32 {
    let Some(d) = trino_platform_pc::tnl_oracle::compare(r, frame) else {
        return 0;
    };
    eprintln!("{name}: {d}");
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/pc-gpu-tnl");
    std::fs::create_dir_all(&dir).unwrap();
    let (w, h, img) = d.side_by_side();
    write_png(&dir.join(format!("{name}.png")), w, h, &img);
    d.assert_within(name);
    d.gpu_draws
}

#[test]
fn three_cards_match_the_golden_per_profile() {
    let _gpu = gpu();
    let golden = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../tests/golden/pc");
    let wide = Camera3 {
        eye: Vec3::new(0.0, 0.0, -2.2),
        ..CAMERA
    };
    for (profile, name, tol) in [
        (SimProfile::Native, "texture_filter_native", 3u8),
        (SimProfile::N3ds, "texture_filter_n3ds", 3),
        (SimProfile::N64, "texture_filter_n64", 9),
    ] {
        let Some(mut r) = headless(profile) else {
            return;
        };
        setup(&mut r);
        let cards = [
            (
                -1.1,
                ModelParams {
                    filter: TexFilter::Point,
                    ..base()
                },
            ),
            (
                0.0,
                ModelParams {
                    filter: TexFilter::Bilinear,
                    ..base()
                },
            ),
            (
                1.1,
                ModelParams {
                    filter: TexFilter::Bilinear,
                    detail: Some(DetailTexture::new(GRAIN, 2)),
                    ..base()
                },
            ),
        ];
        let img = frame(&mut r, &cards, &wide);
        let (w, h) = r.internal_size();
        let path = golden.join(format!("{name}.png"));
        if std::env::var("TRINO_BLESS").is_ok() {
            write_png(&path, w, h, &img);
            continue;
        }
        assert!(path.exists(), "missing {} — bless it", path.display());
        let (gw, gh, want) = read_png(&path);
        assert_eq!((gw, gh), (w, h));
        let bad = img
            .iter()
            .zip(&want)
            .filter(|(a, b)| a.abs_diff(**b) > tol)
            .count();
        if bad > 512 {
            write_png(&golden.join(format!("actual/{name}.png")), w, h, &img);
            panic!("{name}: {bad} channel values off by more than {tol}");
        }
        if profile == SimProfile::Native {
            let gpu = tnl_oracle(&mut r, name, |r| frame(r, &cards, &wide));
            assert_eq!(
                gpu, 3,
                "textured cards, with and without detail, on the GPU"
            );
        }
    }
}
