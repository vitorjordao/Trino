//! The build's thread budget (issue #101): a bake over N threads writes the
//! same files, byte for byte, and the same report (entries and errors, in
//! the same order) as a serial one.

mod common;

use std::collections::BTreeMap;
use std::path::Path;

use common::*;
use trino_asset_pipeline::loader::{BundleLoad, LoadedAssets};
use trino_asset_pipeline::{Platform, bake_all_with};

fn project(root: &Path) {
    let mut manifest = String::from("version = 1\n");
    for i in 0..12u32 {
        write_png_with(
            &root.join(format!("shared/sprites/s{i}.png")),
            32,
            32,
            |x, y| [(x * 8) as u8, (y * 8) as u8, (i * 20) as u8, 255],
        );
        manifest.push_str(&format!("\n[sprites.s{i}]\nfile = \"sprites/s{i}.png\"\n"));
    }
    for i in 0..6u32 {
        let samples: Vec<i16> = (0..8000)
            .map(|n| ((n as f32 * 0.01 * (i + 1) as f32).sin() * 12000.0) as i16)
            .collect();
        write_wav_i16(
            &root.join(format!("shared/sounds/w{i}.wav")),
            22050,
            1,
            &samples,
        );
        manifest.push_str(&format!("\n[sounds.w{i}]\nfile = \"sounds/w{i}.wav\"\n"));
    }
    for i in 0..6u32 {
        write_triangle_gltf(&root.join(format!("shared/models/m{i}.gltf")));
        manifest.push_str(&format!("\n[models.m{i}]\nfile = \"models/m{i}.gltf\"\n"));
    }
    // A broken asset in the middle: its error keeps its place.
    manifest.push_str("\n[sprites.missing]\nfile = \"sprites/nope.png\"\n");
    std::fs::write(root.join("manifest.toml"), manifest).unwrap();
}

fn files(dir: &Path) -> BTreeMap<String, Vec<u8>> {
    std::fs::read_dir(dir)
        .unwrap()
        .map(|e| e.unwrap().path())
        .filter(|p| p.is_file())
        .map(|p| {
            (
                p.file_name().unwrap().to_string_lossy().into_owned(),
                std::fs::read(&p).unwrap(),
            )
        })
        .collect()
}

#[test]
fn a_parallel_bake_matches_the_serial_one_byte_for_byte() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("assets");
    project(&root);
    for platform in [Platform::Pc, Platform::Web] {
        let serial_dir = tmp.path().join(format!("{platform:?}-1"));
        let serial = bake_all_with(&root, platform, &serial_dir, 1).unwrap_err();
        for threads in [2, 8] {
            let dir = tmp.path().join(format!("{platform:?}-{threads}"));
            let parallel = bake_all_with(&root, platform, &dir, threads).unwrap_err();
            assert_eq!(
                format!("{parallel}"),
                format!("{serial}"),
                "same errors, same order ({threads} threads)"
            );
            assert_eq!(
                files(&dir),
                files(&serial_dir),
                "{platform:?}, {threads} threads"
            );
        }
    }
    // Without the broken asset, the reports are equal too.
    let manifest = std::fs::read_to_string(root.join("manifest.toml")).unwrap();
    std::fs::write(
        root.join("manifest.toml"),
        manifest.replace("\n[sprites.missing]\nfile = \"sprites/nope.png\"\n", ""),
    )
    .unwrap();
    let a = bake_all_with(&root, Platform::Pc, &tmp.path().join("ok-1"), 1).unwrap();
    let b = bake_all_with(&root, Platform::Pc, &tmp.path().join("ok-8"), 8).unwrap();
    let key = |r: &trino_asset_pipeline::BakeReport| {
        r.entries
            .iter()
            .map(|e| (e.logical.clone(), e.file.clone(), e.format.clone()))
            .collect::<Vec<_>>()
    };
    assert_eq!(key(&a), key(&b));
}

/// What a load produced, in order: kind, id and the decoded bytes.
fn summary(a: &LoadedAssets) -> Vec<(char, u32, Vec<u8>)> {
    let mut v: Vec<(char, u32, Vec<u8>)> = Vec::new();
    v.extend(a.sprites.iter().map(|s| ('s', s.id, s.rgba.clone())));
    v.extend(
        a.sounds
            .iter()
            .map(|s| ('a', s.id, s.frames.to_le_bytes().to_vec())),
    );
    v.extend(a.models.iter().map(|m| ('m', m.id, m.tmdl.clone())));
    v
}

#[test]
fn a_group_decodes_the_same_on_any_thread_count() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path().join("assets");
    project(&root);
    let manifest = std::fs::read_to_string(root.join("manifest.toml")).unwrap();
    // Everything but the broken sprite in one streamed group.
    let manifest = manifest
        .replace("\n[sprites.missing]\nfile = \"sprites/nope.png\"\n", "")
        .replace(".png\"\n", ".png\"\ngroups = [\"level\"]\n")
        .replace(".wav\"\n", ".wav\"\ngroups = [\"level\"]\n")
        .replace(".gltf\"\n", ".gltf\"\ngroups = [\"level\"]\n")
        + "\n[groups.level]\n";
    std::fs::write(root.join("manifest.toml"), manifest).unwrap();
    let out = tmp.path().join("pc");
    bake_all_with(&root, Platform::Pc, &out, 4).unwrap();
    let bundle = out.join("groups/level.tbnd");
    let load = |threads: u32| {
        let mut b = BundleLoad::open(&bundle, trino_core::HwTier::Base).unwrap();
        let mut all = Vec::new();
        let mut steps = 0;
        while !b.is_done() {
            let mut out = LoadedAssets::default();
            b.step_many(threads, &mut out).unwrap();
            all.extend(summary(&out));
            steps += 1;
        }
        assert_eq!(b.progress().0, b.progress().1);
        (all, steps)
    };
    let (serial, serial_steps) = load(1);
    assert_eq!(serial.len(), 24, "12 sprites, 6 sounds, 6 models");
    let (parallel, steps) = load(8);
    assert_eq!(parallel, serial, "same assets, same order");
    assert!(steps < serial_steps, "{steps} steps vs {serial_steps}");
}
