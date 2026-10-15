//! Particle effects as text, end to end: the RON both ways, validation,
//! the `[particles.*]` bake per platform (each platform's cap, or off), a
//! deterministic simulation (same seed, same particles), and `save`
//! declaring an effect once.

use trino_asset_pipeline::particles::{self, ParticlesDef, PlatformCap};
use trino_asset_pipeline::{Platform, bake_all, load_dir};
use trino_core::Effect;

fn sparks() -> ParticlesDef {
    let mut d = ParticlesDef::new("sprites/spark");
    d.rate = 60.0;
    d.life = (0.5, 1.5);
    d.gravity = [0.0, -9.0, 0.0];
    d.color = ([255, 220, 100, 255], [255, 60, 0, 0]);
    d.frames = 4;
    d.frame_size = [8.0, 8.0];
    d.max = 40;
    d.platforms.n64 = Some(PlatformCap {
        max: Some(10),
        disabled: false,
    });
    d.platforms.n3ds = Some(PlatformCap {
        max: None,
        disabled: true,
    });
    d
}

#[test]
fn effects_round_trip_through_ron_both_ways() {
    let d = sparks();
    let text = d.to_ron();
    assert!(
        text.contains("n64: Some(") && text.contains("max: Some(10)"),
        "{text}"
    );
    assert!(text.contains("disabled: true"));
    assert_eq!(ParticlesDef::from_ron(&text).unwrap(), d);
    assert_eq!(
        ParticlesDef::from_ron(&text).unwrap().to_ron(),
        text,
        "stable"
    );
    // Hand-written, with defaults.
    let hand =
        ParticlesDef::from_ron(r#"(version: 1, sprite: "sprites/dust", rate: 5.0)"#).unwrap();
    assert_eq!(hand.rate, 5.0);
    assert_eq!((hand.max, hand.spread, hand.life), (32, 17.0, (1.0, 1.0)));
    assert!(hand.platforms.is_empty());
    assert_eq!(ParticlesDef::from_ron(&hand.to_ron()).unwrap(), hand);
}

#[test]
fn bad_effects_are_refused_with_the_field() {
    for (text, needle) in [
        (r#"(version: 1, sprite: "s", life: (0.0, 1.0))"#, "life"),
        (r#"(version: 1, sprite: "s", life: (2.0, 1.0))"#, "life"),
        (r#"(version: 1, sprite: "s", speed: (-1.0, 1.0))"#, "speed"),
        (r#"(version: 1, sprite: "s", rate: -1.0)"#, "rate"),
        (r#"(version: 1, sprite: "s", spread: 200.0)"#, "spread"),
        (
            r#"(version: 1, sprite: "s", direction: (0.0, 0.0, 0.0))"#,
            "direction",
        ),
        (r#"(version: 1, sprite: "s", max: 9999)"#, "max 9999"),
        (r#"(version: 1, sprite: "s", frames: 4)"#, "frame_size"),
        (
            r#"(version: 1, sprite: "s", platforms: (n64: Some((max: Some(2000)))))"#,
            "max 2000",
        ),
        (r#"(version: 7, sprite: "s")"#, "newer"),
        (r#"(sprite: "s")"#, "parse error"),
    ] {
        let err = ParticlesDef::from_ron(text).unwrap_err();
        assert!(err.contains(needle), "{text}: {err}");
    }
}

#[test]
fn each_platform_bakes_its_own_cap() {
    let d = sparks();
    assert_eq!(
        (
            d.cap(Platform::Pc),
            d.cap(Platform::N64),
            d.cap(Platform::N3ds)
        ),
        (40, 10, 0)
    );
    for p in Platform::ALL {
        let fx = Effect::parse(&d.tpfx(p)).unwrap();
        assert_eq!(fx.max, d.cap(p));
        assert_eq!(fx.emitter, d.emitter(), "the same effect everywhere");
    }
    // Through the real bake of a project.
    let tmp = tempfile::tempdir().unwrap();
    let assets = tmp.path().join("assets");
    std::fs::create_dir_all(&assets).unwrap();
    std::fs::write(assets.join("manifest.toml"), "version = 1\n").unwrap();
    assert!(particles::save(&assets, "sparks", &d).unwrap(), "declared");
    assert!(
        !particles::save(&assets, "sparks", &d).unwrap(),
        "only once"
    );
    assert!(particles::save(&assets, "Bad Name", &d).is_err());
    let out = tmp.path().join("baked");
    bake_all(&assets, Platform::Pc, &out).unwrap();
    let loaded = load_dir(&out, None).unwrap();
    let id = trino_core::BlobId::from_path("particles/sparks").0;
    let blob = loaded.blobs.iter().find(|b| b.id == id).unwrap();
    assert_eq!(Effect::parse(&blob.bytes).unwrap().max, 40);
    // A platform override file wins, like any master.
    let ovr = assets.join("n64/particles");
    std::fs::create_dir_all(&ovr).unwrap();
    let mut n64 = d.clone();
    n64.rate = 5.0;
    std::fs::write(ovr.join("sparks.particles.ron"), n64.to_ron()).unwrap();
    let path = trino_asset_pipeline::resolve_source(
        &assets,
        Platform::N64,
        "particles/sparks.particles.ron",
    )
    .unwrap();
    let fx = Effect::parse(&particles::bake(&path, Platform::N64).unwrap()).unwrap();
    assert_eq!((fx.emitter.rate, fx.max), (5.0, 10));
}

#[test]
fn the_simulation_is_deterministic_and_keeps_each_cap() {
    let d = sparks();
    let a = particles::simulate(&d, Platform::Pc, 3.0, 7);
    let b = particles::simulate(&d, Platform::Pc, 3.0, 7);
    assert_eq!(a, b, "same seed, same particles");
    let c = particles::simulate(&d, Platform::Pc, 3.0, 8);
    assert_ne!(a.hash, c.hash, "another seed, other particles");
    assert_eq!(a.peak, 40, "60/s for 3 s fills the cap");
    assert_eq!(a.peak_tris, 80);
    let n64 = particles::simulate(&d, Platform::N64, 3.0, 7);
    assert_eq!((n64.cap, n64.peak), (10, 10));
    assert!(n64.peak_n64_us < a.peak_n64_us);
    let off = particles::simulate(&d, Platform::N3ds, 3.0, 7);
    assert_eq!((off.cap, off.peak, off.live), (0, 0, 0), "off on the 3DS");
    // A slow emitter never reaches its cap.
    let mut slow = d.clone();
    slow.rate = 2.0;
    slow.life = (1.0, 1.0);
    let s = particles::simulate(&slow, Platform::Pc, 3.0, 1);
    assert!(s.peak <= 3, "{s:?}");
}
