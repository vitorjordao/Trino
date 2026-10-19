//! Per-language font atlases in their own asset group (issue #59, `[i18n]
//! groups = true`) at run time on the PC host: the language's table names
//! the group and the atlas, the atlas is not resident until the game loads
//! the group through `trino_runner_pc::GroupLoader` (the same loader the
//! runner pumps), and unloading it frees it again. GPU-gated like the
//! goldens (`TRINO_REQUIRE_GPU=1`).

use std::path::{Path, PathBuf};

use trino_core::i18n::{LangId, Localizer};
use trino_core::{FontId, GroupId, LoadState, Platform, Renderer};
use trino_platform_pc::{PcAudio, PcPlatform, PcRenderer, SimProfile};
use trino_runner_pc::GroupLoader;

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn project(dir: &Path) {
    for d in ["assets/i18n", "assets/shared/fonts"] {
        std::fs::create_dir_all(dir.join(d)).unwrap();
    }
    std::fs::copy(
        root().join("assets/shared/fonts/RobotoCondensed-Bold.ttf"),
        dir.join("assets/shared/fonts/roboto.ttf"),
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/manifest.toml"),
        r#"version = 1

[i18n]
default = "en-US"
groups = true

[fonts.ui]
file = "fonts/roboto.ttf"
size = 11
chars = "auto"
per_language = true
"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/i18n/en-US.ron"),
        r#"Strings(lang: "en-US", entries: {"hello": "Hello"})"#,
    )
    .unwrap();
    std::fs::write(
        dir.join("assets/i18n/pt-BR.ron"),
        r#"Strings(lang: "pt-BR", fallback: "en-US", entries: {"hello": "Olá, ação"})"#,
    )
    .unwrap();
}

#[test]
fn a_languages_atlas_loads_and_unloads_with_its_group() {
    let tmp = tempfile::tempdir().unwrap();
    project(tmp.path());
    let baked = tmp.path().join("baked");
    trino_asset_pipeline::bake_all(
        &tmp.path().join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
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
    let mut audio = PcAudio::new_offline(48_000);
    let mut platform = PcPlatform::new(&profile.caps());
    trino_runner_pc::upload_assets(&baked, &mut renderer, &mut audio, &mut platform, None).unwrap();
    let mut loader = GroupLoader::new(&baked, &renderer, &mut platform);

    let mut loc: Localizer<'static, 2> = Localizer::new();
    assert_eq!(loc.load(&platform), 2);
    let base = FontId::from_path("fonts/ui");
    assert!(
        renderer.measure_text(base, "Hello", 1.0).x > 0.0,
        "boot atlas"
    );
    assert!(loc.set_language(LangId::new("pt-BR")));
    let group = GroupId::from_name("i18n-pt-br");
    assert_eq!(loc.group(), Some(group));
    let pt = loc.font(base);
    assert_eq!(pt, FontId::from_path("fonts/ui@pt-BR"));
    let text = loc.tr(trino_core::StrKey::new("hello"));
    assert_eq!(platform.group_state(group), LoadState::Unloaded);
    assert_eq!(
        renderer.measure_text(pt, text, 1.0).x,
        0.0,
        "the pt-BR atlas is not resident before its group loads"
    );

    let mut frames = 0;
    while platform.load_group(group) != LoadState::Ready {
        loader.pump(&mut renderer, &mut audio, &mut platform);
        frames += 1;
        assert!(frames < 600, "the group never became ready");
    }
    assert!(renderer.measure_text(pt, text, 1.0).x > 0.0, "loaded");

    platform.unload_group(group);
    loader.pump(&mut renderer, &mut audio, &mut platform);
    assert_eq!(platform.group_state(group), LoadState::Unloaded);
    assert_eq!(renderer.measure_text(pt, text, 1.0).x, 0.0, "freed");
    // The default language never needed the group.
    loc.set_language(LangId::new("en-US"));
    assert_eq!(loc.group(), None);
    assert_eq!(loc.font(base), base);
}
