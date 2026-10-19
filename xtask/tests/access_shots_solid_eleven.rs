//! Evidence for issue #60 in Solid Eleven, written to `target/evidence/access/`
//! (a separate test binary: two example games can't link into one).
//!
//! ```sh
//! cargo test -p xtask --test access_shots_solid_eleven -- --ignored
//! ```
//!
//! FC Rouge (red) against Verde Rovers (green): the kits normal vision picks,
//! as a deuteranope sees them, and the kits `dress_for` picks for that player,
//! as they see them; the match's captions (the kick-off whistle); and the
//! ball without and with the high-contrast outline.

#[allow(dead_code)]
#[path = "support/solid_eleven.rs"]
mod support;

use solid_eleven::render::Sky;
use trino_core::{AccessSettings, CaptionMode, CvdMode, Game, InputState, Platform};
use trino_platform_pc::{PcPlatform, SimProfile};

struct Mute;
impl trino_core::Audio for Mute {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

/// Solid Eleven's baked blobs (`access/data`, `i18n/*`) on a PC platform.
fn platform() -> PcPlatform {
    let baked = support::root().join("target/assets/solid-eleven/pc");
    let assets = trino_asset_pipeline::load_dir(&baked, None).unwrap();
    let mut p = PcPlatform::new(&SimProfile::Native.caps());
    for b in assets.blobs {
        p.insert_blob(trino_core::BlobId(b.id), b.bytes);
    }
    p
}

#[test]
#[ignore = "evidence: writes target/evidence/access (needs a GPU)"]
fn solid_eleven_kits_and_captions_for_a_deuteranope() {
    let Some(mut r) = support::try_renderer(SimProfile::Native, Some((960, 540))) else {
        return;
    };
    let out = support::root().join("target/evidence/access");
    let mut p = platform();
    let idle = InputState::default();
    // FC Rouge is club 0, Verde Rovers club 2.
    for (cvd, tag) in [
        (CvdMode::Off, "normal"),
        (CvdMode::Deuteranopia, "deuteranopia"),
    ] {
        p.set_access(AccessSettings {
            cvd,
            captions: CaptionMode::All,
            ..AccessSettings::DEFAULT
        });
        let mut g = solid_eleven::SolidElevenGame::new(trino_core::Vec2::new(960.0, 540.0));
        g.update_with(&mut p, &idle, &mut Mute, 1.0 / 60.0);
        g.start_exhibition(0, 2, 5, [false, false], Sky::Day, 180);
        for _ in 0..90 {
            g.update_with(&mut p, &idle, &mut Mute, 1.0 / 60.0);
        }
        for (view, seen) in [
            (CvdMode::Off, "drawn"),
            (CvdMode::Deuteranopia, "seen_deuteranopia"),
        ] {
            r.set_cvd_view(view);
            g.render(&mut r);
            let (w, h) = r.internal_size();
            support::write_png(
                &out.join(format!("solid_eleven_kits_{tag}_{seen}.png")),
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
fn solid_eleven_ball_outline_off_and_on() {
    let Some(mut r) = support::try_renderer(SimProfile::Native, Some((960, 540))) else {
        return;
    };
    let out = support::root().join("target/evidence/access");
    let mut p = platform();
    let idle = InputState::default();
    for (high_contrast, tag) in [(false, "off"), (true, "on")] {
        p.set_access(AccessSettings {
            high_contrast,
            ..AccessSettings::DEFAULT
        });
        let mut g = solid_eleven::SolidElevenGame::new(trino_core::Vec2::new(960.0, 540.0));
        g.update_with(&mut p, &idle, &mut Mute, 1.0 / 60.0);
        g.start_exhibition(0, 2, 5, [false, false], Sky::Day, 180);
        for _ in 0..90 {
            g.update_with(&mut p, &idle, &mut Mute, 1.0 / 60.0);
        }
        g.render(&mut r);
        let (w, h) = r.internal_size();
        support::write_png(
            &out.join(format!("solid_eleven_ball_outline_{tag}.png")),
            w,
            h,
            &r.read_offscreen(),
        );
    }
}

/// Solid Eleven's menus after the box check (`solid_eleven_menus`): the
/// longest club name on the title's card and the quick match's, the club
/// screen's buttons in Portuguese; and the pause menu (a UI-system screen)
/// at 100 % and 200 % text in both languages.
#[test]
#[ignore = "evidence: writes target/evidence/access (needs a GPU)"]
fn solid_eleven_menus_fit_their_boxes() {
    use solid_eleven::game::app::Scene;
    use solid_eleven::ui::i18n::Lang;
    let Some(mut r) = support::try_renderer(SimProfile::N64, None) else {
        return;
    };
    let out = support::root().join("target/evidence/access");
    let mut p = platform();
    let idle = InputState::default();
    let shot = |g: &mut solid_eleven::SolidElevenGame,
                r: &mut trino_platform_pc::PcRenderer,
                name: &str| {
        g.render(r);
        let (w, h) = r.internal_size();
        support::write_png(
            &out.join(format!("solid_eleven_{name}.png")),
            w,
            h,
            &r.read_offscreen(),
        );
    };
    let fresh = |p: &mut PcPlatform, lang: Lang, text_scale: u8| {
        p.set_access(AccessSettings {
            text_scale,
            ..AccessSettings::DEFAULT
        });
        let mut g = solid_eleven::SolidElevenGame::new(trino_core::Vec2::new(320.0, 240.0));
        for _ in 0..4 {
            g.update_with(p, &idle, &mut Mute, 1.0 / 30.0);
        }
        g.settings.lang = lang;
        g
    };
    // Violet Wanderers (club 6) on the title's card and the quick match.
    let mut g = fresh(&mut p, Lang::Pt, 100);
    g.quick.home = 6;
    g.quick.away = 9;
    shot(&mut g, &mut r, "title_card_long_name");
    g.scene = Scene::Quick;
    shot(&mut g, &mut r, "quick_long_names");
    // The club screen's buttons in Portuguese.
    let mut g = fresh(&mut p, Lang::Pt, 100);
    g.world = solid_eleven::sim::league::Career::new(6, 9);
    g.career_on = true;
    g.scene = Scene::Club;
    shot(&mut g, &mut r, "club_buttons_pt");
    // The pause menu at both text scales.
    for lang in Lang::ALL {
        for ts in [100u8, 200] {
            let mut g = fresh(&mut p, lang, ts);
            g.start_exhibition(0, 2, 5, [true, false], Sky::Day, 180);
            for _ in 0..10 {
                g.update_with(&mut p, &idle, &mut Mute, 1.0 / 30.0);
            }
            let mut start = InputState::default();
            start.set(trino_core::Button::Start, true);
            g.update_with(&mut p, &start, &mut Mute, 1.0 / 30.0);
            g.update_with(&mut p, &idle, &mut Mute, 1.0 / 30.0);
            assert_eq!(g.scene, Scene::Pause);
            shot(
                &mut g,
                &mut r,
                &format!("pause_{lang:?}_{ts}").to_lowercase(),
            );
        }
    }
}
