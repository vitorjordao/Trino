//! E2E hot-reload test: build the game dylib twice (v1 = as-is, v2 = source
//! patched to double the movement speed), load both into this process and
//! call them **against the same state** — proving that
//!
//! 1. the exported ABI (`trino_game_*`) is loadable and callable,
//! 2. the api-version handshake works,
//! 3. game state owned by the host survives a library swap while behavior
//!    changes — the core promise of live reload.
//!
//! Slow (two cargo builds), so `#[ignore]`d locally; CI runs it in the
//! dedicated hot-reload job via `cargo test -p xtask -- --ignored`.

use std::path::{Path, PathBuf};
use std::process::Command;

use trino_core::{Audio, Button, InputState, MusicId, SoundId, Vec2};

struct NullAudio;
impl Audio for NullAudio {
    fn play_sound(&mut self, _: SoundId) {}
    fn play_music(&mut self, _: MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

type VersionFn = fn() -> u32;
type UpdateFn = fn(&mut hello_sprite::HelloGame, &InputState, &mut dyn Audio, f32);

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn dylib_name() -> String {
    format!(
        "{}hello_sprite{}",
        std::env::consts::DLL_PREFIX,
        std::env::consts::DLL_SUFFIX
    )
}

fn cargo() -> String {
    std::env::var("CARGO").unwrap_or_else(|_| "cargo".into())
}

/// Build a patched copy of hello-sprite (SPEED doubled) in a temp dir,
/// standalone from the workspace, and return the dylib path.
fn build_v2(temp: &Path) -> PathBuf {
    let root = repo_root().canonicalize().unwrap();
    let core = root.join("crates/core");
    let api = root.join("crates/game-api");

    let src_dir = temp.join("src");
    std::fs::create_dir_all(&src_dir).unwrap();

    let original = std::fs::read_to_string(root.join("examples/hello-sprite/src/lib.rs")).unwrap();
    let patched = original.replace("const SPEED: f32 = 120.0", "const SPEED: f32 = 240.0");
    assert_ne!(original, patched, "SPEED constant not found to patch");
    std::fs::write(src_dir.join("lib.rs"), patched).unwrap();

    let manifest = format!(
        r#"[package]
name = "hello-sprite"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["dylib"]

[dependencies]
trino-core = {{ path = {core:?} }}
trino-game-api = {{ path = {api:?} }}

[workspace]
"#,
        core = core.to_string_lossy(),
        api = api.to_string_lossy(),
    );
    std::fs::write(temp.join("Cargo.toml"), manifest).unwrap();

    let status = Command::new(cargo())
        .args(["build"])
        .current_dir(temp)
        .status()
        .expect("failed to run cargo for v2 dylib");
    assert!(status.success(), "v2 dylib build failed");
    temp.join("target/debug").join(dylib_name())
}

#[test]
#[ignore = "slow: builds the game dylib twice; CI runs it in the hot-reload job"]
fn state_survives_dylib_swap_and_behavior_changes() {
    let root = repo_root();

    // v1: the workspace's own dylib.
    let status = Command::new(cargo())
        .args(["build", "-p", "hello-sprite"])
        .current_dir(&root)
        .status()
        .expect("failed to build hello-sprite");
    assert!(status.success());
    let v1_path = root.join("target/debug").join(dylib_name());
    assert!(v1_path.exists(), "dylib not found at {}", v1_path.display());

    // v2: patched copy, built standalone.
    let temp = tempfile::tempdir().unwrap();
    let v2_path = build_v2(temp.path());

    let mut audio = NullAudio;
    let mut input = InputState::default();
    input.set(Button::DpadRight, true);

    // Host-owned state, created via the statically-linked crate.
    let mut game = hello_sprite::HelloGame::new(Vec2::new(320.0, 240.0));
    let x0 = game.pos.x;

    unsafe {
        let v1 = libloading::Library::new(&v1_path).expect("failed to load v1 dylib");
        let version: libloading::Symbol<VersionFn> =
            v1.get(b"trino_game_api_version").expect("version symbol");
        assert_eq!(version(), trino_game_api::GAME_API_VERSION);

        let update: libloading::Symbol<UpdateFn> =
            v1.get(b"trino_game_update").expect("update symbol");
        // dt = 0.1 keeps the sprite far from the screen-edge clamp.
        update(&mut game, &input, &mut audio, 0.1);
    }
    let x1 = game.pos.x;
    assert_eq!(x1 - x0, 12.0, "v1 moves at SPEED=120");

    unsafe {
        let v2 = libloading::Library::new(&v2_path).expect("failed to load v2 dylib");
        let version: libloading::Symbol<VersionFn> =
            v2.get(b"trino_game_api_version").expect("version symbol");
        assert_eq!(version(), trino_game_api::GAME_API_VERSION);

        let update: libloading::Symbol<UpdateFn> =
            v2.get(b"trino_game_update").expect("update symbol");
        // SAME state object, new code.
        update(&mut game, &input, &mut audio, 0.1);
    }
    let x2 = game.pos.x;
    assert_eq!(
        x2 - x1,
        24.0,
        "v2 moves at SPEED=240 with the v1 state intact"
    );
}
