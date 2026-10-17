//! Nintendo 64 host: boot, the game, the main loop (`n64_main!`), and the
//! self-test `cargo trino test n64` runs in ares: a scripted match, timed in
//! emulated N64 time and checked against the PC's sim hash.

#![cfg_attr(target_os = "none", no_std)]

#[cfg(target_os = "none")]
extern crate alloc;

// RSP streaming: draws go to the RSP in chunks while the CPU builds the
// rest of the frame (a match is ~100 RSP draws: the RSP and the CPU then
// work at the same time instead of one after the other).
trino_platform_n64::n64_main!(
    {{game_new}},
    self_test,
    |host: &mut trino_platform_n64::N64Host| host.renderer.set_rsp_stream(true)
);

/// Runs in ares under `cargo trino test n64`. `Err` fails the test.
#[cfg(target_os = "none")]
fn self_test(
    host: &mut trino_platform_n64::N64Host,
    game: &mut {{name_snake}}::{{name_camel}}Game,
) -> Result<(), alloc::string::String> {
    use {{name_snake}}::selftest;
    // Ten seconds of match at 30 fps (two sim ticks a frame), a human's
    // buttons from the script: `trino.toml`'s `n64_max_frame_us` gates the
    // average frame.
    selftest::begin(game);
    let report = host.measure(game, selftest::FRAMES, selftest::DT, selftest::input);
    host.report_frames("match", &report);
    host.log(&alloc::format!("TRINO_SE_HASH:{:08x}\n", selftest::hash(game)));
    // The game's own render-side CPU (posing, culling, HUD layout): the
    // same frame drawn into a renderer that draws nothing.
    let mut null = selftest::NullRenderer {
        caps: trino_core::Caps::N64,
    };
    let t0 = trino_platform_n64::runtime::now_us();
    for _ in 0..10 {
        trino_core::Game::render(game, &mut null);
    }
    let us = trino_platform_n64::runtime::now_us().wrapping_sub(t0) / 10;
    host.log(&alloc::format!("TRINO_SE_RENDER_CPU:{us}\n"));
    selftest::check(game).map_err(alloc::string::String::from)?;
    // The golden frame: the match as the script left it.
    host.step(game, &trino_core::InputState::default(), 0.0);
    host.dump_framebuffer();
    Ok(())
}
