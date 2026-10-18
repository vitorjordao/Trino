//! Scene streaming's base (issue #56 phase 6): portal cells declare asset
//! groups (`cell(groups: …)`, scene v6), and `trino_core::CellGroups` keeps
//! those of the rooms in view, and a doorway further, loaded on the PC
//! backend's real groups (`trino_runner_pc::GroupLoader`), letting the rest
//! go. GPU-gated like the goldens (`TRINO_REQUIRE_GPU=1`).

use std::path::PathBuf;

use trino_core::{Camera3, GroupId, LoadState, Platform, Vec2, Vec3};
use trino_platform_pc::{PcAudio, PcPlatform, PcRenderer, SimProfile};
use trino_runner_pc::GroupLoader;
use trino_scene::{Entity, Library, Scene, VisComponent, resolve};

/// Three rooms in a row along x (8 wide), doorways at x = 8 and 16: the
/// hall streams `selftest_level`, the kitchen `selftest_level2`, the yard
/// nothing.
fn house() -> Scene {
    let mut s = Scene::new("house");
    for (name, x, groups) in [
        ("hall", 4.0, vec!["selftest_level".to_string()]),
        ("kitchen", 12.0, vec!["selftest_level2".to_string()]),
        ("yard", 20.0, vec![]),
    ] {
        let mut room = Entity::new(name).at([x, 2.0, 4.0]);
        room.vis = Some(VisComponent::Cell {
            size: [8.0, 4.0, 8.0],
            groups,
        });
        s.entities.push(room);
    }
    for (name, x, a, b) in [
        ("door1", 8.0, "hall", "kitchen"),
        ("door2", 16.0, "kitchen", "yard"),
    ] {
        let mut door = Entity::new(name).at([x, 1.0, 4.0]);
        door.vis = Some(VisComponent::Portal {
            size: [0.2, 2.0, 1.5],
            a: a.into(),
            b: b.into(),
        });
        s.entities.push(door);
    }
    s
}

#[test]
fn cells_stream_their_groups_on_pc() {
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..");
    let baked = tempfile::tempdir().unwrap();
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        baked.path(),
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let profile = SimProfile::N64;
    let mut renderer = match pollster::block_on(PcRenderer::new_headless(profile)) {
        Ok(r) => r,
        Err(e) => {
            assert!(std::env::var("TRINO_REQUIRE_GPU").is_err(), "{e}");
            eprintln!("skipping: {e}");
            return;
        }
    };
    renderer.set_strict(true);
    let mut audio = PcAudio::new_offline(48_000);
    let mut platform = PcPlatform::new(&profile.caps());
    trino_runner_pc::upload_assets(baked.path(), &mut renderer, &mut audio, &mut platform, None)
        .unwrap();
    let mut loader = GroupLoader::new(baked.path(), &renderer, &mut platform);

    let resolved = resolve(&house(), &Library::default(), 0.0);
    assert!(resolved.warnings.is_empty(), "{:?}", resolved.warnings);
    let cells = resolved.cells();
    let (mut streamer, dropped) = resolved.cell_streamer::<4>(10);
    assert!(dropped.is_empty(), "{dropped:?}");
    let (hall, kitchen) = (
        GroupId::from_name("selftest_level"),
        GroupId::from_name("selftest_level2"),
    );
    let screen = Vec2::new(320.0, 240.0);
    let cam = |eye: [f32; 3], target: [f32; 3]| Camera3 {
        eye: Vec3::new(eye[0], eye[1], eye[2]),
        target: Vec3::new(target[0], target[1], target[2]),
        fov_y: 1.0,
    };

    // Frames as a game runs them: the host pumps, the game streams.
    let mut run = |camera: &Camera3, frames: u32, platform: &mut PcPlatform| -> (u64, bool) {
        let visible = cells.visible(camera, screen);
        let mut ready = false;
        for _ in 0..frames {
            loader.pump(&mut renderer, &mut audio, platform);
            ready = streamer.stream(platform, &cells, visible);
        }
        (visible, ready)
    };

    // In the hall, facing the back wall: only the hall is in view, the
    // kitchen is a doorway away — both load, the hall's ready to draw.
    let (visible, ready) = run(&cam([2.0, 1.6, 4.0], [-4.0, 1.6, 4.0]), 120, &mut platform);
    assert_eq!(visible & 0b111, 0b001, "{visible:b}");
    assert!(ready);
    assert_eq!(platform.group_state(hall), LoadState::Ready);
    assert_eq!(
        platform.group_state(kitchen),
        LoadState::Ready,
        "prefetched"
    );

    // Out in the yard, facing away from the house: the kitchen (its
    // neighbour) stays, the hall lingers 10 frames and goes.
    let (visible, ready) = run(&cam([22.0, 1.6, 4.0], [30.0, 1.6, 4.0]), 5, &mut platform);
    assert_eq!(visible & 0b111, 0b100, "{visible:b}");
    assert!(ready, "the yard needs no group");
    assert_eq!(platform.group_state(hall), LoadState::Ready, "lingers");
    run(&cam([22.0, 1.6, 4.0], [30.0, 1.6, 4.0]), 10, &mut platform);
    loader.pump(&mut renderer, &mut audio, &mut platform);
    assert_eq!(platform.group_state(hall), LoadState::Unloaded);
    assert_eq!(platform.group_state(kitchen), LoadState::Ready);
    assert!(!streamer.holds(hall) && streamer.holds(kitchen));

    // Leaving the level lets everything go: boot alone again.
    streamer.release(&mut platform);
    loader.pump(&mut renderer, &mut audio, &mut platform);
    assert_eq!(platform.group_state(kitchen), LoadState::Unloaded);
    assert_eq!(platform.memory().groups_resident, 1);
}
