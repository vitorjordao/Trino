//! Strict mode on the N64 profile checks TMEM by each texture's N64 format
//! (GitHub #125), end to end: the PC bake records the format the N64 bake
//! chooses (`n64_format` in the index), `upload_assets` hands it to the
//! renderer, and a 3D draw of the texture panics exactly when
//! `N64Format::fits_tmem` says the console can't hold it — a CI4/CI8
//! texture's texels have TMEM's lower 2 KB, a 4-bit one takes half a
//! 16-bit one's bytes. GPU-gated like the goldens (`TRINO_REQUIRE_GPU=1`).

#[path = "../../platform-pc/tests/common/mod.rs"]
mod common;

use std::panic::{AssertUnwindSafe, catch_unwind};

use trino_asset_pipeline::{N64Format, Platform as Target};
use trino_core::render3d::{Camera3, DEFAULT_LIGHT};
use trino_core::{BillboardParams, Color, Renderer, SpriteId, Vec3};
use trino_platform_pc::{PcAudio, PcPlatform, PcRenderer, SimProfile};
use trino_runner_pc::upload_assets;

/// Sizes on both sides of each format's room: (format, width, height).
/// 72×40 CI4 is `fonts/trino5x7`'s atlas before #99 (it read as 71 % of
/// TMEM at 16 bits a texel); 64×64 I4 fits where 16-bit texels would not.
const CASES: &[(&str, u32, u32)] = &[
    ("CI4", 72, 40),
    ("CI4", 64, 64),
    ("CI4", 64, 65),
    ("CI4", 17, 9),
    ("CI8", 32, 64),
    ("CI8", 48, 48),
    ("I4", 64, 64),
    ("I4", 64, 128),
    ("I4", 128, 65),
    ("I8", 64, 64),
    ("I8", 64, 65),
    ("I8", 3, 7),
    ("RGBA5551", 32, 64),
    ("RGBA5551", 48, 48),
    ("RGBA5551", 1, 256),
];

fn name(format: &str, w: u32, h: u32) -> String {
    format!("t_{}_{w}x{h}", format.to_lowercase())
}

/// One sprite per case, two colors (a palette of any size holds them).
fn project(assets: &std::path::Path) {
    let mut manifest = String::from("version = 1\n");
    for &(format, w, h) in CASES {
        let n = name(format, w, h);
        let px: Vec<u8> = (0..w * h)
            .flat_map(|i| {
                if i % 3 == 0 {
                    [255, 255, 255, 255]
                } else {
                    [0, 0, 0, 255]
                }
            })
            .collect();
        common::write_png(&assets.join(format!("shared/sprites/{n}.png")), w, h, &px);
        manifest.push_str(&format!(
            "\n[sprites.{n}]\nfile = \"sprites/{n}.png\"\nformats = {{ n64 = \"{format}\" }}\n"
        ));
    }
    std::fs::write(assets.join("manifest.toml"), manifest).unwrap();
}

fn draw(r: &mut PcRenderer, sprite: SpriteId) {
    r.begin_frame(Color::rgb(0, 0, 0));
    r.set_camera(&Camera3 {
        eye: Vec3::new(0.0, 0.0, -3.0),
        target: Vec3::ZERO,
        fov_y: 1.0,
    });
    r.set_light(&DEFAULT_LIGHT);
    r.draw_billboard(sprite, Vec3::ZERO, &BillboardParams::DEFAULT);
    r.end_frame();
}

#[test]
fn strict_mode_on_the_n64_profile_agrees_with_tmem_texels() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let baked = dir.path().join("baked");
    project(&assets);
    trino_asset_pipeline::bake_all(&assets, Target::Pc, &baked).unwrap_or_else(|e| panic!("{e}"));
    // The bake records each sprite's N64 format.
    let index = std::fs::read_to_string(baked.join("index.toml")).unwrap();
    for &(format, _, _) in CASES {
        assert!(
            index.contains(&format!("n64_format = \"{format}\"")),
            "{index}"
        );
    }
    let formats = trino_asset_pipeline::texture_formats(&baked).unwrap();
    assert_eq!(formats.len(), CASES.len());

    let Some(mut r) = common::headless(SimProfile::N64) else {
        return;
    };
    let mut audio = PcAudio::new_offline(48_000);
    let mut platform = PcPlatform::new(&SimProfile::N64.caps());
    upload_assets(&baked, &mut r, &mut audio, &mut platform, None).unwrap();
    r.set_strict(true);
    let mut table = String::from("format    size     texels / room   tools   strict\n");
    for &(format, w, h) in CASES {
        let sprite = common::sprite(&format!("sprites/{}", name(format, w, h)));
        let f = N64Format::parse(format).unwrap();
        let (texels, room) = f.tmem_texels(w, h);
        let fits = f.fits_tmem(w, h);
        let drew = catch_unwind(AssertUnwindSafe(|| draw(&mut r, sprite))).is_ok();
        table.push_str(&format!(
            "{format:<9} {:<8} {texels:>6} / {room:<6} {:<7} {}\n",
            format!("{w}x{h}"),
            if fits { "fits" } else { "over" },
            if drew { "draws" } else { "panics" }
        ));
        assert_eq!(drew, fits, "{format} {w}x{h}: {texels} of {room} bytes");
    }
    eprintln!("{table}");
    let out =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/evidence/strict-tmem");
    std::fs::create_dir_all(&out).unwrap();
    std::fs::write(out.join("strict_tmem.txt"), &table).unwrap();

    // Without the bake's formats the check is the old 16-bit one: the
    // 72×40 CI4 atlas (5.6 KB at 16 bits) is rejected, though it fits.
    let ci4 = common::sprite(&format!("sprites/{}", name("CI4", 72, 40)));
    r.set_texture_format(ci4, None);
    assert!(catch_unwind(AssertUnwindSafe(|| draw(&mut r, ci4))).is_err());
}

/// The formats follow the N64's own master: an override under
/// `assets/n64/` decides the format the PC's strict mode checks.
#[test]
fn the_n64_format_comes_from_the_n64_master() {
    let dir = tempfile::tempdir().unwrap();
    let assets = dir.path().join("assets");
    let baked = dir.path().join("baked");
    // A 64×64 master with many colors; `auto` on the N64 picks by measure.
    let rich: Vec<u8> = (0..64 * 64u32)
        .flat_map(|i| [(i * 7) as u8, (i * 13) as u8, (i * 29) as u8, 255])
        .collect();
    common::write_png(&assets.join("shared/sprites/a.png"), 64, 64, &rich);
    // The N64's override is gray: an intensity format keeps it.
    let gray: Vec<u8> = (0..64 * 64u32)
        .flat_map(|i| {
            let v = (i % 64 * 4) as u8;
            [v, v, v, 255]
        })
        .collect();
    common::write_png(&assets.join("n64/sprites/a.png"), 64, 64, &gray);
    std::fs::write(
        assets.join("manifest.toml"),
        "version = 1\n\n[sprites.a]\nfile = \"sprites/a.png\"\nformats = { n64 = \"auto\" }\n",
    )
    .unwrap();
    trino_asset_pipeline::bake_all(&assets, Target::Pc, &baked).unwrap_or_else(|e| panic!("{e}"));
    let chosen = trino_asset_pipeline::texture::choose(
        Target::N64,
        "sprites/a",
        trino_asset_pipeline::texture::Source::Sprite,
        &trino_asset_pipeline::Manifest::load(&assets.join("manifest.toml"))
            .unwrap()
            .sprites["a"]
            .settings(Target::N64),
        64,
        64,
        &gray,
    )
    .unwrap();
    let index = std::fs::read_to_string(baked.join("index.toml")).unwrap();
    assert!(
        index.contains(&format!("n64_format = \"{}\"", chosen.format)),
        "{} expected from the override: {index}",
        chosen.format
    );
}
