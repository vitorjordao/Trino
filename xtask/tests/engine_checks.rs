//! `castle64::selftest::engine_checks` on the PC backend — the same checks
//! the N64/3DS self-tests run inside ares/Azahar. GPU-gated like the
//! goldens (`TRINO_REQUIRE_GPU=1` turns a missing adapter into a failure).

use std::path::PathBuf;

use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

#[test]
fn engine_checks_pass_on_pc() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(baked.path(), None).unwrap();
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(SimProfile::N64)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    renderer.set_strict(true);
    let save_dir = tempfile::tempdir().unwrap();
    let mut platform = PcPlatform::new(&SimProfile::N64.caps())
        .with_save_file(save_dir.path().join("selftest.sav"));
    for s in assets.sprites {
        renderer.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for f in assets.fonts {
        renderer.upload_font(trino_core::FontId(f.id), f.table);
    }
    for b in assets.blobs {
        platform.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    castle64::selftest::engine_checks(&mut renderer, &mut platform).unwrap();
}
