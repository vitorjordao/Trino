//! Nintendo 3DS host: boot, the game, the main loop (`n3ds_main!`), and the
//! self-test `cargo trino test 3ds` runs in Azahar: the scripted match,
//! checked against the PC's sim hash, both screens dumped.

#![cfg_attr(target_os = "horizon", no_std)]
#![cfg_attr(target_os = "horizon", no_main)]

#[cfg(target_os = "horizon")]
extern crate alloc;

trino_platform_3ds::n3ds_main!({{game_new}}, self_test);

/// Runs in Azahar under `cargo trino test 3ds`. `Err` fails the test.
#[cfg(target_os = "horizon")]
fn self_test(
    host: &mut trino_platform_3ds::N3dsHost,
    game: &mut {{name_snake}}::{{name_camel}}Game,
) -> Result<(), alloc::string::String> {
    use {{name_snake}}::selftest;
    selftest::begin(game);
    let report = host.measure(game, selftest::FRAMES, selftest::DT, selftest::input);
    host.report_frames("match", &report);
    host.log(&alloc::format!("TRINO_SE_HASH:{:08x}\n", game.session.m.hash()));
    selftest::check(game).map_err(alloc::string::String::from)?;
    // The LCDs are double-buffered: present the same frame a few times.
    for _ in 0..3 {
        host.step(game, &trino_core::InputState::default(), 0.0);
    }
    host.dump_framebuffer();
    host.dump_framebuffer_bottom();
    Ok(())
}
