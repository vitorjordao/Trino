//! Nintendo 64 host: boot, the game, the main loop (`n64_main!`), and the
//! self-test `cargo trino test n64` runs in ares.

#![cfg_attr(target_os = "none", no_std)]

#[cfg(target_os = "none")]
extern crate alloc;

trino_platform_n64::n64_main!({{game_new}}, self_test);

/// Runs in ares under `cargo trino test n64`. `Err` fails the test.
#[cfg(target_os = "none")]
fn self_test(
    host: &mut trino_platform_n64::N64Host,
    game: &mut {{name_snake}}::{{name_camel}}Game,
) -> Result<(), alloc::string::String> {
    // Two seconds of play with no input, timed in emulated N64 time:
    // `trino.toml`'s `n64_max_frame_us` gates the average.
    let report = host.measure(game, 120, 1.0 / 60.0, |_| Default::default());
    host.report_frames("idle", &report);
    Ok(())
}
