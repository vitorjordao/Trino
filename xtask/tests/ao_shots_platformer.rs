//! Ambient occlusion evidence for the legacy platformer (its spinning cube is the only 3D), in a binary of its own (each
//! example exports the hot-reload symbols). See `ao_shots.rs`:
//!
//! ```sh
//! cargo test -p xtask --test ao_shots_platformer -- --ignored
//! ```

mod ao_evidence;

#[test]
#[ignore = "evidence: writes target/evidence/ao (needs a GPU)"]
fn ambient_occlusion_evidence() {
    let mut game = platformer::PlatformerGame::new(ao_evidence::native_screen());
    ao_evidence::game_row("platformer", &mut game);
}
