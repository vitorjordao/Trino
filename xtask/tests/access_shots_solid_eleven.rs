//! Evidence for issue #60 in Solid Eleven, written to `target/evidence/access/`
//! (a separate test binary: two example games can't link into one).
//!
//! ```sh
//! cargo test -p xtask --test access_shots_solid_eleven -- --ignored
//! ```
//!
//! FC Rouge (red) against Verde Rovers (green): the kits normal vision picks,
//! as a deuteranope sees them, and the kits `dress_for` picks for that player,
//! as they see them; and the match's captions (the kick-off whistle).

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
