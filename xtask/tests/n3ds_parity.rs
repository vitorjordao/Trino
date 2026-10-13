//! PC "3DS look" vs the real thing: castle64's hub perf scene rendered by
//! the PC backend (N3ds profile) is compared with the frame Azahar produced
//! from the self-test app (`tests/golden/3ds/castle64_hub.png`, written by
//! `cargo xtask test 3ds --bless`). Different rasterizers (and Azahar's GPU
//! output depends on the host's OpenGL), so the test bounds the drift
//! instead of matching pixels: a backend that loses a model, breaks a
//! texture or gets depth wrong over large areas fails here. GPU-gated like
//! the goldens; `TRINO_PARITY_DUMP=1` writes the PC frame next to the golden.

use std::path::PathBuf;

use trino_core::{Game, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn read_png(path: &std::path::Path) -> (u32, u32, Vec<u8>) {
    let mut dec = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    dec.set_transformations(png::Transformations::EXPAND);
    let mut reader = dec.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    buf.truncate(info.buffer_size());
    (info.width, info.height, buf)
}

#[test]
fn pc_3ds_look_tracks_the_azahar_frame() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let golden = root.join("tests/golden/3ds/castle64_hub.png");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::N3ds)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    let hub = &castle64::perf::SCENES[0];
    let mut game = castle64::perf::scene_game(hub, Vec2::new(400.0, 240.0), &mut NullAudio);
    game.render(&mut r);
    // The console dump is RGBA5551: quantize the same way.
    let expand = |c: u8| ((c >> 3) << 3) | (c >> 5);
    let pc: Vec<u8> = r
        .read_offscreen()
        .chunks(4)
        .flat_map(|p| [expand(p[0]), expand(p[1]), expand(p[2]), 255])
        .collect();
    let (w, h, azahar) = read_png(&golden);
    assert_eq!((w, h), r.internal_size());
    if std::env::var_os("TRINO_PARITY_DUMP").is_some() {
        let out = root.join("tests/golden/3ds/actual/pc_look.png");
        std::fs::create_dir_all(out.parent().unwrap()).unwrap();
        let mut enc = png::Encoder::new(std::fs::File::create(&out).unwrap(), w, h);
        enc.set_color(png::ColorType::Rgba);
        enc.set_depth(png::BitDepth::Eight);
        enc.write_header().unwrap().write_image_data(&pc).unwrap();
    }
    let diffs: Vec<u8> = pc
        .chunks(4)
        .zip(azahar.chunks(4))
        .map(|(a, b)| (0..3).map(|i| a[i].abs_diff(b[i])).max().unwrap())
        .collect();
    let n = diffs.len() as f32;
    let mean = diffs.iter().map(|&d| d as f32).sum::<f32>() / n;
    let far = diffs.iter().filter(|&&d| d > 32).count() as f32 / n;
    eprintln!(
        "pc vs azahar: mean max-channel diff {mean:.2}, {:.2}% of pixels differ by > 32",
        far * 100.0
    );
    // Measured 0.60 mean / 1.00% far (triangle edges, HUD glyph filtering)
    // when written. Before castle64 set its own light it was 24.5 / 31%:
    // the console renderer still held the self-test showcase's fogged
    // light — the kind of drift this test exists to catch.
    assert!(mean < 2.5, "mean diff {mean:.2}");
    assert!(far < 0.025, "{:.2}% of pixels far apart", far * 100.0);
}
