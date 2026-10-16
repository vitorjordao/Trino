//! Shared by the Solid Eleven render tests: a headless PC renderer with
//! the game's own baked assets, and PNG writing.

use std::path::{Path, PathBuf};

use trino_platform_pc::{PcRenderer, SimProfile};

pub fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// A headless renderer with Solid Eleven's assets, or `None` without a
/// GPU adapter (an error with `TRINO_REQUIRE_GPU`).
pub fn try_renderer(profile: SimProfile, size: Option<(u32, u32)>) -> Option<PcRenderer> {
    let root = root();
    let baked = root.join("target/assets/solid-eleven/pc");
    trino_asset_pipeline::bake_all(
        &root.join("examples/solid-eleven/assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            if std::env::var("TRINO_REQUIRE_GPU").is_ok() {
                panic!("TRINO_REQUIRE_GPU set but no adapter: {e}");
            }
            eprintln!("skipping: no GPU adapter ({e})");
            return None;
        }
    };
    if let Some((w, h)) = size {
        r.set_internal_size(w, h);
    }
    if profile == SimProfile::N64 {
        r.set_n64_look(true);
    }
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    Some(r)
}

/// [`try_renderer`] for tools that need a GPU.
pub fn renderer(profile: SimProfile, size: Option<(u32, u32)>) -> PcRenderer {
    try_renderer(profile, size).expect("a GPU adapter")
}

pub fn write_png(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    let file = std::fs::File::create(path).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(rgba).unwrap();
}
