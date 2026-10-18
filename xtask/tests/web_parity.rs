//! The Web's picture is the PC's (issue #47): the browser goldens
//! (`tests/golden/web/*.png`, written by `cargo xtask test web --bless`
//! from a browser's self-test) against the same frames rendered here by the
//! PC backend on the Web profile, with the Web's own assets (its bake, each
//! KTX2 texture decoded by Binomial's transcoder as a browser without
//! compressed textures shows it). Both run `trino-render-wgpu`, so only the
//! rasterizer (SwiftShader / a browser's GPU path vs this machine's) and the
//! KTX2 block re-encoding may differ: the test bounds that drift, and a
//! backend that loses a model, breaks a texture or gets depth wrong fails
//! here. GPU-gated like the goldens; `TRINO_PARITY_DUMP=1` writes the PC
//! frames into the goldens' `actual/`.

use std::path::{Path, PathBuf};

use trino_core::{Game, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// The browser goldens' size (`apps/web/src/selftest.rs` `GOLDEN`).
const SIZE: (u32, u32) = (640, 480);
/// The browser harness's own tolerance (`xtask/src/web.rs`).
const MAX_MEAN: f32 = 3.0;
const MAX_FAR: f32 = 0.02;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn read_png(path: &Path) -> (u32, u32, Vec<u8>) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

fn write_png(out: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(out.parent().unwrap()).unwrap();
    let mut enc = png::Encoder::new(std::fs::File::create(out).unwrap(), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}

/// Mean of each pixel's largest channel difference, and the share above 32
/// (`xtask/src/fbgolden.rs` `distance`).
fn distance(a: &[u8], b: &[u8]) -> (f32, f32) {
    let diffs: Vec<u8> = a
        .chunks(4)
        .zip(b.chunks(4))
        .map(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap())
        .collect();
    let n = diffs.len().max(1) as f32;
    let mean = diffs.iter().map(|&d| d as f32).sum::<f32>() / n;
    let far = diffs.iter().filter(|&&d| d > 32).count() as f32 / n;
    (mean, far)
}

/// A headless PC renderer on the Web profile with the Web's bake. `None`
/// (skip) without a GPU.
fn web_renderer(root: &Path) -> Option<PcRenderer> {
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Web,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::Web)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return None;
        }
    };
    r.set_internal_size(SIZE.0, SIZE.1);
    let mut ktx2 = 0;
    for s in assets.sprites {
        let px = match &s.ktx2 {
            Some(file) => {
                ktx2 += 1;
                trino_asset_pipeline::texture::ktx2::decode(file).unwrap()
            }
            None => s.rgba.clone(),
        };
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &px);
    }
    assert!(ktx2 > 0, "the Web bake made no KTX2 texture");
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    Some(r)
}

/// The browser self-test's golden frames, drawn the same way.
fn frame(r: &mut PcRenderer, name: &str) -> Vec<u8> {
    let screen = Vec2::new(SIZE.0 as f32, SIZE.1 as f32);
    match name {
        "showcase" => {
            let mut g = Box::new(showcase::ShowcaseGame::blank());
            showcase::perf::river_scene(&mut g, screen, &mut NullAudio);
            for _ in 0..3 {
                g.render(r);
            }
        }
        "castle64_hub" => {
            let hub = &castle64::perf::SCENES[0];
            let mut g = castle64::perf::scene_game(hub, screen, &mut NullAudio);
            for _ in 0..3 {
                g.render(r);
            }
        }
        "texture_formats" => castle64::selftest::draw_texture_formats(r, true),
        other => panic!("no frame {other}"),
    }
    r.read_offscreen()
}

#[test]
fn the_browsers_frames_are_the_pc_renderers() {
    let root = root();
    let Some(mut r) = web_renderer(&root) else {
        return;
    };
    let dump = std::env::var("TRINO_PARITY_DUMP").is_ok();
    let mut failures = Vec::new();
    for name in ["showcase", "castle64_hub", "texture_formats"] {
        let golden = root.join("tests/golden/web").join(format!("{name}.png"));
        let (w, h, want) = read_png(&golden);
        assert_eq!((w, h), SIZE, "{}", golden.display());
        let got = frame(&mut r, name);
        let (mean, far) = distance(&got, &want);
        println!(
            "{name}: PC (Web profile) vs browser golden: mean {mean:.2}, {:.2}% far",
            far * 100.0
        );
        if dump {
            let out = root
                .join("tests/golden/web/actual")
                .join(format!("{name}_pc.png"));
            write_png(&out, w, h, &got);
        }
        if mean > MAX_MEAN || far > MAX_FAR {
            failures.push(format!("{name}: mean {mean:.2}, {:.2}% far", far * 100.0));
        }
    }
    assert!(
        failures.is_empty(),
        "the PC renderer drifted from the browser goldens: {failures:?}"
    );
}
