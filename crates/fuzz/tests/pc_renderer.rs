//! What the `pc_renderer` target made the PC renderer survive (issue
//! #121): uploads a damaged or modded asset file can ask for are refused
//! and logged, not handed to wgpu's validation (which aborts).

use trino_core::{Color, Renderer, SpriteId, SpriteParams, Vec2};
use trino_platform_pc::{CompressedFormat, PcRenderer, SimProfile};

#[test]
fn uploads_the_device_cant_make_are_refused_not_aborted() {
    let mut r = match pollster::block_on(PcRenderer::new_headless(SimProfile::Native)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    let id = SpriteId(7);
    // Zero sides, a side past the device's limit, a length that isn't the
    // size, and a size whose byte count wraps 32 bits.
    r.upload_sprite(id, 0, 0, &[]);
    r.upload_sprite(id, 4, 0, &[]);
    r.upload_sprite(id, 8193, 1, &vec![0; 8193 * 4]);
    r.upload_sprite(id, 2, 2, &[255; 15]);
    r.upload_sprite(id, 1 << 15, 1 << 15, &[]);
    if r.supports_bc() {
        for (w, h, len) in [(13, 16, 104), (8, 8, 31), (0, 8, 0), (8192 + 4, 4, 0)] {
            assert!(
                !r.upload_sprite_bc(id, w, h, CompressedFormat::Bc1, &vec![0; len]),
                "{w}x{h} with {len} bytes"
            );
        }
        assert!(r.upload_sprite_bc(id, 8, 8, CompressedFormat::Bc1, &[0; 32]));
    }
    // A valid one still uploads and draws.
    r.upload_sprite(id, 2, 2, &[255; 16]);
    r.begin_frame(Color::BLACK);
    r.draw_sprite(id, Vec2::new(4.0, 4.0), &SpriteParams::DEFAULT);
    r.end_frame();
}
