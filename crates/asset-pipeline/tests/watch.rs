//! Live reload (`watch` feature): saving a master — or a scene that the
//! manifest bakes — rebakes in the background and reports exactly the
//! handles whose baked bytes changed. A save that breaks the bake is
//! reported and the watcher keeps going.

#![cfg(feature = "watch")]

mod common;

use std::sync::mpsc;
use std::time::{Duration, Instant};

use common::*;
use trino_asset_pipeline::watch::watch;
use trino_asset_pipeline::{Platform, bake_all, load_dir};
use trino_core::asset_id;

/// Next reported batch, or `None` after `timeout`.
fn next(rx: &mpsc::Receiver<Vec<u32>>, timeout: Duration) -> Option<Vec<u32>> {
    rx.recv_timeout(timeout).ok()
}

/// Wait until a batch containing `id` arrives (other batches are skipped:
/// editors may write in several steps).
fn wait_for(rx: &mpsc::Receiver<Vec<u32>>, id: u32) -> Vec<u32> {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        if let Some(batch) = next(rx, deadline - Instant::now())
            && batch.contains(&id)
        {
            return batch;
        }
    }
    panic!("no rebake reported handle {id:#010x} within 20 s");
}

#[test]
fn saving_a_master_or_a_scene_rebakes_only_what_changed() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let out = dir.path().join("out");
    write_png(&assets.join("shared/sprites/a.png"), 1, 1, [255, 0, 0, 255]);
    write_png(&assets.join("shared/sprites/b.png"), 1, 1, [0, 0, 255, 255]);
    std::fs::create_dir_all(dir.path().join("scenes")).unwrap();
    let scene = dir.path().join("scenes/level.scene.ron");
    std::fs::write(
        &scene,
        r#"(version: 2, name: "level", entities: [(name: "start")])"#,
    )
    .unwrap();
    std::fs::write(
        assets.join("manifest.toml"),
        r#"version = 1
[sprites.a]
file = "sprites/a.png"
[sprites.b]
file = "sprites/b.png"
[scenes.level]
file = "scenes/level.scene.ron"
"#,
    )
    .unwrap();
    bake_all(&assets, Platform::Pc, &out).unwrap();

    let (tx, rx) = mpsc::channel();
    let guard = match watch(assets.clone(), Platform::Pc, out.clone(), move |ids| {
        let _ = tx.send(ids);
    }) {
        Ok(g) => g,
        Err(e) => {
            eprintln!("skipping: file watching unavailable here: {e}");
            return;
        }
    };

    // A master edit: only that sprite's handle, with the new pixels baked.
    let a = asset_id("sprites/a");
    write_png(&assets.join("shared/sprites/a.png"), 1, 1, [0, 255, 0, 255]);
    assert_eq!(wait_for(&rx, a), vec![a]);
    let baked = load_dir(&out, Some(&[a])).unwrap();
    assert_eq!(baked.sprites[0].rgba, [0, 255, 0, 255]);

    // A half-written master fails the rebake; the watcher survives it and
    // picks up the fixed file.
    std::fs::write(assets.join("shared/sprites/b.png"), b"\x89PNG trunc").unwrap();
    // Past the debounce window: the broken save gets a rebake of its own.
    std::thread::sleep(Duration::from_millis(600));
    let b = asset_id("sprites/b");
    write_png(&assets.join("shared/sprites/b.png"), 1, 1, [9, 9, 9, 255]);
    assert_eq!(wait_for(&rx, b), vec![b]);

    // Scenes live next to assets/: the watcher covers them too. Let the
    // sprite saves' trailing events settle first, so the scene edit alone
    // triggers this rebake.
    std::thread::sleep(Duration::from_millis(800));
    std::fs::write(
        &scene,
        r#"(version: 2, name: "level", entities: [(name: "start"), (name: "exit")])"#,
    )
    .unwrap();
    let level = asset_id("scenes/level");
    assert_eq!(wait_for(&rx, level), vec![level]);
    let blob = load_dir(&out, Some(&[level])).unwrap().blobs.remove(0);
    let parsed = trino_core::BakedScene::parse(Box::leak(blob.bytes.into_boxed_slice())).unwrap();
    assert_eq!(parsed.node_count(), 2);
    assert!(parsed.find("exit").is_some());

    // Dropping the guard stops watching: later edits report nothing.
    drop(guard);
    while next(&rx, Duration::from_millis(400)).is_some() {}
    write_png(&assets.join("shared/sprites/a.png"), 1, 1, [1, 1, 1, 255]);
    assert_eq!(next(&rx, Duration::from_secs(1)), None);
}
