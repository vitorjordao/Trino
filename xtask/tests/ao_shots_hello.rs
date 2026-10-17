//! Ambient occlusion evidence for hello-sprite (2D only: AO leaves it as it was), in a binary of its own (each
//! example exports the hot-reload symbols). See `ao_shots.rs`:
//!
//! ```sh
//! cargo test -p xtask --test ao_shots_hello -- --ignored
//! ```

mod ao_evidence;

#[test]
#[ignore = "evidence: writes target/evidence/ao (needs a GPU)"]
fn ambient_occlusion_evidence() {
    let mut game = hello_sprite::HelloGame::new(ao_evidence::native_screen());
    ao_evidence::game_row("hello_sprite", &mut game);
}
