//! Pose sheets: every Solid Eleven clip, frame by frame, from the front,
//! the side and the back, on the PC's 11-part player and the consoles'
//! 7-part one — for looking at the body's fit (sleeves, shorts, neck)
//! through each animation. Not a gate (the gate is the geometric fit test
//! in `examples/solid-eleven/src/render/fit.rs`):
//!
//! ```sh
//! cargo test -p xtask --test solid_eleven_poses -- --ignored --nocapture
//! ```
//!
//! Writes `target/solid_eleven_poses/<clip>_<tier>.png`.

use std::path::PathBuf;

use solid_eleven::render::anim::AnimState;
use solid_eleven::render::clips_gen::{CLIPS, ClipName};
use solid_eleven::render::studio;
use solid_eleven::render::{Detail, Look, SKIN_TONES};
use solid_eleven::sim::team::Rgb;
use trino_platform_pc::SimProfile;

#[path = "support/solid_eleven.rs"]
#[allow(dead_code)]
mod support;

const CELL: (u32, u32) = (240, 320);
const TIMES: usize = 8;
/// Facing per row: front-left three-quarters, side, back.
const VIEWS: [(f32, f32); 3] = [(0.5, 0.866), (1.0, 0.0), (-0.42, -0.906)];

#[test]
#[ignore = "writes PNGs for review; run with --ignored"]
fn pose_sheets() {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/solid_eleven_poses");
    std::fs::create_dir_all(&out).unwrap();
    // FC Rouge's home kit (its baked console legs too).
    let look = Look::of(
        Rgb(206, 32, 41),
        Rgb(240, 240, 240),
        Rgb(206, 32, 41),
        SKIN_TONES[3],
    );
    for (profile, tier, detail) in [
        (SimProfile::Native, "pc", Detail::Full),
        (SimProfile::N64, "n64", Detail::Near),
    ] {
        let mut r = support::renderer(profile, Some(CELL));
        for name in ClipName::ALL {
            let clip = &CLIPS[name as usize];
            let (w, h) = (CELL.0 * TIMES as u32, CELL.1 * VIEWS.len() as u32);
            let mut sheet = vec![0u8; (w * h * 4) as usize];
            for (row, face) in VIEWS.iter().enumerate() {
                for col in 0..TIMES {
                    let t = if clip.looped {
                        clip.seconds * col as f32 / TIMES as f32
                    } else {
                        clip.seconds * col as f32 / (TIMES - 1) as f32
                    };
                    let mut a = AnimState::new(name);
                    a.time = t;
                    let pose = a.pose_masked(detail.mask());
                    studio::figure(
                        &mut r,
                        &pose,
                        &look,
                        *face,
                        detail,
                        (0.5, 7.5),
                        clip.grounded,
                    );
                    trino_core::Renderer::end_frame(&mut r);
                    let px = r.read_offscreen();
                    for y in 0..CELL.1 {
                        let src = (y * CELL.0 * 4) as usize;
                        let dst =
                            (((row as u32 * CELL.1 + y) * w + col as u32 * CELL.0) * 4) as usize;
                        sheet[dst..dst + (CELL.0 * 4) as usize]
                            .copy_from_slice(&px[src..src + (CELL.0 * 4) as usize]);
                    }
                }
            }
            let path = out.join(format!("{}_{tier}.png", clip.name));
            support::write_png(&path, w, h, &sheet);
            eprintln!("pose sheet: {}", path.display());
        }
    }
}
