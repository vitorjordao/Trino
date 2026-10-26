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
    // First, on the fresh game: the PC's two-player recording (issue #67),
    // every hash compared.
    selftest::replay_two_players(&mut host.platform, game)
        .map_err(alloc::string::String::from)?;
    // What rollback netplay would cost here (the 3DS local-wireless
    // evaluation, issue #67): snapshot, load and update times.
    selftest::begin(game);
    if let Some(cost) = host.rollback_cost(game, 60, selftest::DT, selftest::input) {
        let mut line = alloc::string::String::new();
        let _ = cost.write_line(&mut line);
        host.log(&alloc::format!("{line}\n"));
    }
    selftest::begin(game);
    let report = host.measure(game, selftest::FRAMES, selftest::DT, selftest::input);
    host.report_frames("match", &report);
    host.log(&alloc::format!("TRINO_SE_HASH:{:08x}\n", selftest::hash(game)));
    selftest::check(game).map_err(alloc::string::String::from)?;
    // The LCDs are double-buffered: present the same frame a few times.
    for _ in 0..3 {
        host.step(game, &trino_core::InputState::default(), 0.0);
    }
    host.dump_framebuffer();
    host.dump_framebuffer_bottom();
    // The match on in stereo (issue #63), after the goldens (it plays on):
    // the slider fixed at full, both eyes per frame, reported only
    // (`stereo=` says if the Auto policy dropped an eye).
    host.renderer.set_stereo_slider_override(Some(1.0));
    let stereo = host.measure(game, selftest::FRAMES, selftest::DT, selftest::input);
    host.report_frames("match-stereo", &stereo);
    host.renderer.set_stereo_slider_override(None);
    Ok(())
}
