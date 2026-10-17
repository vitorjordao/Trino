//! Baked vertex ambient occlusion (`ao` on a `[models.*]` entry, ADR 0014):
//! the consoles' AO tier, written into the vertex colors by the bake.

use std::path::PathBuf;

use trino_asset_pipeline::{AoBake, Manifest, ModelOptions, bake_model};
use trino_core::Mesh;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..")
}

/// The self-test probe: a white panel between two fins.
fn probe() -> PathBuf {
    root().join("assets/shared/models/proxies/selftest-ao.gltf")
}

fn colors(tmdl: &[u8]) -> Vec<([f32; 3], [u8; 4])> {
    let mesh = Mesh::from_tmdl(tmdl).unwrap();
    (0..mesh.vertex_count)
        .map(|i| {
            let p = mesh.position(i);
            let c = mesh.color(i);
            ([p.x, p.y, p.z], [c.r, c.g, c.b, c.a])
        })
        .collect()
}

#[test]
fn the_bake_darkens_the_panel_by_the_fins_and_leaves_its_middle() {
    let raw = bake_model(&probe(), &ModelOptions::default()).unwrap();
    let opts = ModelOptions {
        ao: Some(AoBake {
            strength: 0.9,
            distance: 0.5,
            ..AoBake::default()
        }),
        ..ModelOptions::default()
    };
    let ao = bake_model(&probe(), &opts).unwrap();
    // Same layout, same positions: only colors change.
    assert_eq!(raw.len(), ao.len());
    let (raw, ao) = (colors(&raw), colors(&ao));
    for ((p, a), (q, b)) in raw.iter().zip(&ao) {
        assert_eq!(p, q);
        assert_eq!(a, &[255, 255, 255, 255], "unbaked: white");
        assert_eq!(b[3], 255, "alpha stays");
        assert!(b[0] == b[1] && b[1] == b[2], "gray stays gray");
        // The panel (z = 0): dark at the fins, white in the middle.
        if p[2] == 0.0 && p[0].abs() > 0.49 {
            assert!(b[0] < 190, "{p:?}: {b:?}");
        }
        if p[2] == 0.0 && p[0].abs() < 0.01 {
            assert!(b[0] > 245, "{p:?}: {b:?}");
        }
    }
    // Deterministic: the same bytes again.
    assert_eq!(
        bake_model(&probe(), &opts).unwrap(),
        bake_model(&probe(), &opts).unwrap()
    );
}

#[test]
fn the_engine_manifest_bakes_the_probe_with_and_without_ao() {
    let manifest = Manifest::load(&root().join("assets/manifest.toml")).unwrap();
    let with = &manifest.models["selftest_ao"];
    let without = &manifest.models["selftest_ao_raw"];
    assert_eq!(with.file, without.file);
    let ao = with.options().ao.expect("ao on selftest_ao");
    assert_eq!((ao.strength, ao.distance), (0.9, 0.5));
    assert!(without.options().ao.is_none());
    // `ao = false` is no bake, `ao = true` the defaults.
    let text = r#"
version = 1
[models.a]
file = "models/a.glb"
ao = false
[models.b]
file = "models/b.glb"
ao = true
"#;
    let m: Manifest = toml::from_str(text).unwrap();
    assert!(m.models["a"].options().ao.is_none());
    assert_eq!(m.models["b"].options().ao, Some(AoBake::default()));
}
