//! Nintendo 3DS host: boot, the game, the main loop (`n3ds_main!`), and the
//! self-test `cargo trino test 3ds` runs in Azahar.

#![cfg_attr(target_os = "horizon", no_std)]
#![cfg_attr(target_os = "horizon", no_main)]

#[cfg(target_os = "horizon")]
extern crate alloc;

trino_platform_3ds::n3ds_main!({{name_snake}}::{{name_camel}}Game::new, self_test);

/// Runs in Azahar under `cargo trino test 3ds`. `Err` fails the test.
#[cfg(target_os = "horizon")]
fn self_test(
    host: &mut trino_platform_3ds::N3dsHost,
    game: &mut {{name_snake}}::{{name_camel}}Game,
) -> Result<(), alloc::string::String> {
    let report = host.measure(game, 120, 1.0 / 60.0, |_| Default::default());
    host.report_frames("idle", &report);
    Ok(())
}
