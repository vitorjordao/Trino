//! Evidence for issue #60 (accessibility), written to `target/evidence/access/`
//! (not goldens: the pictures a reviewer looks at). Run with:
//!
//! ```sh
//! cargo test -p xtask --test access_shots -- --ignored
//! cargo test -p xtask --test access_shots_solid_eleven -- --ignored
//! ```
//!
//! - castle64's stage doors in each color-vision mode: as the game draws them
//!   for that player, and as that player sees the base colors and the
//!   alternatives (the renderer's simulation view);
//! - castle64 with a caption up, in normal and high contrast at 200 %;
//! - castle64's hub at spawn (the player and a boar) without and with the
//!   high-contrast outlines;
//! - the starter Accessibility and Options screens at 100 %, 200 % and in high
//!   contrast on the N64, the 3DS and the PC.

#[allow(dead_code)]
#[path = "../src/ui_render.rs"]
mod ui_render;

use std::path::{Path, PathBuf};

use trino_core::{
    AccessSettings, CaptionMode, CvdMode, Family, Game, InputState, Platform, Renderer, Vec2,
    asset_id,
};
use trino_platform_pc::{PcPlatform, PcRenderer, SimProfile};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..")
}

fn out() -> PathBuf {
    let d = root().join("target/evidence/access");
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn save(path: &Path, w: u32, h: u32, rgba: &[u8]) {
    ui_render::write_png(path, w, h, rgba).unwrap();
    eprintln!("evidence: {}", path.display());
}

fn castle64_target(profile: SimProfile, size: Option<(u32, u32)>) -> (PcRenderer, PcPlatform) {
    let root = root();
    let baked = root.join("target/assets/access-evidence");
    trino_asset_pipeline::bake_all(
        &root.join("assets"),
        trino_asset_pipeline::Platform::Pc,
        &baked,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut r = pollster::block_on(PcRenderer::new_headless(profile)).expect("a GPU adapter");
    if let Some((w, h)) = size {
        r.set_internal_size(w, h);
    }
    let mut p = PcPlatform::new(&profile.caps());
    for s in assets.sprites {
        r.upload_sprite(trino_core::SpriteId(s.id), s.width, s.height, &s.rgba);
    }
    for m in assets.models {
        r.upload_mesh(trino_core::ModelId(m.id), m.tmdl);
    }
    for f in assets.fonts {
        r.upload_font(trino_core::FontId(f.id), f.table);
    }
    for b in assets.blobs {
        p.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    (r, p)
}

/// castle64 in the hub, back from the castle so the green (west), red
/// (east) and blue (facade) stage doors are all in view, open (three stars).
fn hub_doors(p: &mut PcPlatform, settings: AccessSettings, screen: Vec2) -> castle64::Castle64Game {
    const DT: f32 = 1.0 / 60.0;
    let mut game = castle64::Castle64Game::new(screen);
    p.set_access(settings);
    let mut audio = NullAudio;
    let idle = InputState::default();
    for _ in 0..30 {
        game.update_with(p, &idle, &mut audio, DT);
    }
    game.stars = 0b111;
    game.pos = trino_core::Vec3::new(0.0, 0.55, -12.0);
    for _ in 0..30 {
        game.update_with(p, &idle, &mut audio, DT);
    }
    game
}

#[test]
#[ignore = "evidence: writes target/evidence/access (needs a GPU)"]
fn castle64_doors_per_color_vision_mode() {
    let (mut r, mut p) = castle64_target(SimProfile::Native, Some((640, 360)));
    let screen = Vec2::new(640.0, 360.0);
    // The base colors, as each viewer sees them.
    let mut base = hub_doors(&mut p, AccessSettings::DEFAULT, screen);
    for mode in CvdMode::ALL {
        r.set_cvd_view(mode);
        base.render(&mut r);
        let (w, h) = r.internal_size();
        save(
            &out().join(format!("castle64_doors_base_seen_{}.png", mode.key())),
            w,
            h,
            &r.read_offscreen(),
        );
    }
    // The alternatives the game draws for each mode, as drawn and as seen.
    for mode in CvdMode::ALL {
        let mut game = hub_doors(
            &mut p,
            AccessSettings {
                cvd: mode,
                ..AccessSettings::DEFAULT
            },
            screen,
        );
        for (view, tag) in [(CvdMode::Off, "drawn"), (mode, "seen")] {
            r.set_cvd_view(view);
            game.render(&mut r);
            let (w, h) = r.internal_size();
            save(
                &out().join(format!("castle64_doors_{}_{tag}.png", mode.key())),
                w,
                h,
                &r.read_offscreen(),
            );
        }
    }
    r.set_cvd_view(CvdMode::Off);
}

#[test]
#[ignore = "evidence: writes target/evidence/access (needs a GPU)"]
fn castle64_captions_normal_and_high_contrast() {
    for (profile, size, tag) in [
        (SimProfile::N64, None, "n64"),
        (SimProfile::Native, Some((1280, 720)), "pc"),
        (SimProfile::N3ds, None, "3ds"),
    ] {
        let (mut r, mut p) = castle64_target(profile, size);
        let (w, h) = r.internal_size();
        for (settings, name) in [
            (
                AccessSettings {
                    captions: CaptionMode::All,
                    ..AccessSettings::DEFAULT
                },
                "captions",
            ),
            (
                AccessSettings {
                    captions: CaptionMode::All,
                    text_scale: 200,
                    high_contrast: true,
                    ..AccessSettings::DEFAULT
                },
                "captions_200_contrast",
            ),
        ] {
            let mut game = hub_doors(&mut p, settings, Vec2::new(w as f32, h as f32));
            game.captions.on_sound(asset_id("sounds/coin"), None);
            game.captions.on_sound(asset_id("sounds/win"), None);
            game.render(&mut r);
            let rgba = match r.second_screen() {
                // Both 3DS screens stacked, as `ui shot` does.
                Some(b) => {
                    let (bw, bh) = (b.x as u32, b.y as u32);
                    let top = r.read_offscreen();
                    let bottom = r.read_second_screen();
                    let mut all = vec![0u8; (w * (h + bh) * 4) as usize];
                    all[..top.len()].copy_from_slice(&top);
                    let x0 = (w - bw) / 2;
                    for y in 0..bh {
                        let from = (y * bw * 4) as usize;
                        let to = (((h + y) * w + x0) * 4) as usize;
                        all[to..to + (bw * 4) as usize]
                            .copy_from_slice(&bottom[from..from + (bw * 4) as usize]);
                    }
                    save(
                        &out().join(format!("castle64_{name}_{tag}.png")),
                        w,
                        h + bh,
                        &all,
                    );
                    continue;
                }
                None => r.read_offscreen(),
            };
            save(
                &out().join(format!("castle64_{name}_{tag}.png")),
                w,
                h,
                &rgba,
            );
        }
    }
}

#[test]
#[ignore = "evidence: writes target/evidence/access (needs a GPU)"]
fn castle64_outlines_off_and_on() {
    const DT: f32 = 1.0 / 60.0;
    for (profile, size, tag) in [
        (SimProfile::N64, None, "n64"),
        (SimProfile::Native, Some((1280, 720)), "pc"),
        (SimProfile::N3ds, None, "3ds"),
    ] {
        let (mut r, mut p) = castle64_target(profile, size);
        let (w, h) = r.internal_size();
        for (high_contrast, name) in [(false, "off"), (true, "on")] {
            p.set_access(AccessSettings {
                high_contrast,
                ..AccessSettings::DEFAULT
            });
            let mut game = castle64::Castle64Game::new(Vec2::new(w as f32, h as f32));
            let mut audio = NullAudio;
            let idle = InputState::default();
            for _ in 0..45 {
                game.update_with(&mut p, &idle, &mut audio, DT);
            }
            game.render(&mut r);
            save(
                &out().join(format!("castle64_outline_{name}_{tag}.png")),
                w,
                h,
                &r.read_offscreen(),
            );
        }
    }
}

#[test]
#[ignore = "evidence: writes target/evidence/access (needs a GPU)"]
fn starter_screens_at_each_text_scale_and_contrast() {
    use trino_scene::ui::{Langs, ScreenDef, StringsDef};
    let root = root();
    let manifest =
        trino_asset_pipeline::Manifest::load(&root.join("assets/manifest.toml")).unwrap();
    let mut langs = Langs::default();
    let t = StringsDef::load(&root.join("templates/standalone/assets/i18n/en-US.ron")).unwrap();
    langs.tables.insert(t.lang.clone(), t);
    for screen in ["accessibility", "options"] {
        let text =
            std::fs::read_to_string(root.join(format!("templates/standalone/ui/{screen}.ui.ron")))
                .unwrap();
        let def = ScreenDef::from_ron(&text.replace("{{name}}", "orbit-cat")).unwrap();
        for family in [Family::N64, Family::N3ds, Family::Pc] {
            for (access, tag) in [
                (trino_core::ui::UiAccess::NONE, "100"),
                (
                    trino_core::ui::UiAccess {
                        text_scale: 2.0,
                        ..trino_core::ui::UiAccess::NONE
                    },
                    "200",
                ),
                (
                    trino_core::ui::UiAccess {
                        text_scale: 2.0,
                        high_contrast: true,
                        ..trino_core::ui::UiAccess::NONE
                    },
                    "200_contrast",
                ),
            ] {
                let shot = ui_render::render(
                    &root,
                    &manifest,
                    &langs,
                    &def,
                    &ui_render::ShotOptions {
                        family,
                        size: Some((1280, 720)),
                        lang: Some("en-US"),
                        interface_scale: 1.0,
                        focus: None,
                        access,
                        simulate: None,
                        stereo: None,
                    },
                )
                .unwrap();
                save(
                    &out().join(format!("ui_{screen}_{}_{tag}.png", family.key())),
                    shot.width,
                    shot.height,
                    &shot.rgba,
                );
            }
        }
    }
}
