//! Solid Eleven's ball shadow on the PC's map tier at several heights
//! (contact hardening and distance attenuation, #86), for looking at the
//! change (not a gate):
//!
//! ```sh
//! cargo test -p xtask --test solid_eleven_shadow_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/solid_eleven_shots/ball_<height>m.png` (or into
//! `$TRINO_SE_SHADOW_SHOTS`): a close camera on the ball, placed at rest
//! and 1, 2.5 and 5 m up over the same spot of turf, on the native profile
//! at 960x540 with 4x MSAA.

use std::path::{Path, PathBuf};

use solid_eleven::SolidElevenGame;
use solid_eleven::render::Sky;
use solid_eleven::sim::fx::Fx;
use trino_core::{Game, InputState, Vec2};
use trino_platform_pc::{PcRenderer, SimProfile};

#[path = "support/solid_eleven.rs"]
#[allow(dead_code)]
mod support;
use support::{renderer, root};

struct NullAudio;
impl trino_core::Audio for NullAudio {
    fn play_sound(&mut self, _: trino_core::SoundId) {}
    fn play_music(&mut self, _: trino_core::MusicId, _: bool) {}
    fn stop_music(&mut self) {}
    fn set_master_volume(&mut self, _: f32) {}
}

fn save(r: &PcRenderer, dir: &Path, name: &str) {
    let (w, h) = r.internal_size();
    let rgba = r.read_offscreen();
    std::fs::create_dir_all(dir).unwrap();
    let out = dir.join(format!("{name}.png"));
    let file = std::fs::File::create(&out).unwrap();
    let mut enc = png::Encoder::new(std::io::BufWriter::new(file), w, h);
    enc.set_color(png::ColorType::Rgba);
    enc.set_depth(png::BitDepth::Eight);
    enc.write_header().unwrap().write_image_data(&rgba).unwrap();
    eprintln!("screenshot: {}", out.display());
}

#[test]
#[ignore = "local tool: screenshots (needs a GPU)"]
fn solid_eleven_ball_shadow_shots() {
    let dir = std::env::var("TRINO_SE_SHADOW_SHOTS")
        .map(PathBuf::from)
        .unwrap_or_else(|_| root().join("target/solid_eleven_shots"));
    let mut r = renderer(SimProfile::Native, Some((960, 540)));
    r.set_msaa(true);
    let (w, h) = r.internal_size();
    let mut g = SolidElevenGame::exhibition(
        Vec2::new(w as f32, h as f32),
        0,
        1,
        7,
        [false, false],
        Sky::Day,
    );
    let mut a = NullAudio;
    for _ in 0..60 * 8 {
        g.update(&InputState::default(), &mut a, 1.0 / 60.0);
    }
    for height in [0.11f32, 1.0, 2.5, 5.0] {
        let s = g.session.as_mut().unwrap();
        // An open spot of turf, the same every time; the ball still.
        s.m.ball.pos.x = Fx::from_f32(-14.0);
        s.m.ball.pos.z = Fx::from_f32(9.0);
        s.m.ball.pos.y = Fx::from_f32(height);
        s.m.ball.vel = Default::default();
        s.view.cam.focus = trino_core::Vec3::new(-14.0, 1.6, 9.0);
        s.view.cam.height = 4.0;
        s.view.cam.back = 10.0;
        g.render(&mut r);
        save(&r, &dir, &format!("ball_{height}m"));
    }
}
