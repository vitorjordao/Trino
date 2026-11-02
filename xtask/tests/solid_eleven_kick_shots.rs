//! Evidence for issue #136 (Solid Eleven's kicks on graph layers), for
//! review. Not a gate (the gate is `a_shot_at_a_sprint_keeps_the_runs_arm_swing`):
//!
//! ```sh
//! cargo test -p xtask --test solid_eleven_kick_shots -- --ignored --nocapture
//! ```
//!
//! Writes `target/evidence/anim136/kick_layers_<tier>.png`: a shot through
//! six moments (columns), seen from the side. Rows:
//! 1. before #136: the shot clip replaces the whole body (the gait stops);
//! 2. the shot laid over a standstill (strike and arms layers);
//! 3. over a jog (the arms take part of it);
//! 4. over a sprint: legs, hips, torso and head kick, the arms keep the
//!    run's swing.

use std::path::PathBuf;

use solid_eleven::render::anim::AnimState;
use solid_eleven::render::clips_gen::ClipName;
use solid_eleven::render::graph::KickLayers;
use solid_eleven::render::{Detail, Look, SKIN_TONES, kick_layer, studio};
use solid_eleven::sim::team::Rgb;
use trino_platform_pc::SimProfile;

#[path = "support/solid_eleven.rs"]
#[allow(dead_code)]
mod support;

const CELL: (u32, u32) = (200, 280);
/// Frames into the kick at which each column is taken (60 Hz).
const FRAMES: [usize; 6] = [4, 10, 16, 22, 28, 34];
/// (gait, ground speed in m/s) per "after" row.
const GAITS: [(ClipName, f32); 3] = [
    (ClipName::Idle, 0.0),
    (ClipName::Jog, 4.6),
    (ClipName::Sprint, 9.0),
];

#[test]
#[ignore = "writes PNGs for review; run with --ignored"]
fn kick_layers() {
    let out = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../target/evidence/anim136");
    std::fs::create_dir_all(&out).unwrap();
    let kicks = KickLayers::load();
    assert_eq!(kicks.clips, 0b1_1111);
    let look = Look::of(
        Rgb(206, 32, 41),
        Rgb(240, 240, 240),
        Rgb(206, 32, 41),
        SKIN_TONES[3],
    );
    let dt = 1.0 / 60.0;
    for (profile, tier, detail) in [
        (SimProfile::Native, "pc", Detail::Full),
        (SimProfile::N64, "n64", Detail::Near),
    ] {
        let mut r = support::renderer(profile, Some(CELL));
        let rows = 1 + GAITS.len();
        let (w, h) = (CELL.0 * FRAMES.len() as u32, CELL.1 * rows as u32);
        let mut sheet = vec![0u8; (w * h * 4) as usize];
        let mut put = |row: usize, col: usize, px: &[u8]| {
            for y in 0..CELL.1 {
                let src = (y * CELL.0 * 4) as usize;
                let dst = (((row as u32 * CELL.1 + y) * w + col as u32 * CELL.0) * 4) as usize;
                sheet[dst..dst + (CELL.0 * 4) as usize]
                    .copy_from_slice(&px[src..src + (CELL.0 * 4) as usize]);
            }
        };
        let mut shoot = |pose: &solid_eleven::render::anim::Pose, grounded: bool| {
            studio::figure(
                &mut r,
                pose,
                &look,
                (1.0, 0.0),
                detail,
                (0.5, 7.5),
                grounded,
            );
            trino_core::Renderer::end_frame(&mut r);
            r.read_offscreen()
        };
        // Row 0: the whole body plays the shot (what the game did before).
        for (col, f) in FRAMES.iter().enumerate() {
            let mut a = AnimState::new(ClipName::Shot);
            a.time = *f as f32 * dt;
            let px = shoot(&a.pose_masked(detail.mask()), true);
            put(0, col, &px);
        }
        // Rows 1..: the gait goes on, the kick is laid over it.
        for (row, (gait, speed)) in GAITS.iter().enumerate() {
            let mut st = AnimState::new(*gait);
            st.speed = *speed;
            let mut col = 0;
            for frame in 1..=*FRAMES.last().unwrap() {
                kick_layer(&mut st, Some(ClipName::Shot), dt);
                st.advance(dt, 1.0);
                if frame == FRAMES[col] {
                    let px = shoot(&st.pose_with(detail.mask(), &kicks), true);
                    put(row + 1, col, &px);
                    col += 1;
                }
            }
        }
        let path = out.join(format!("kick_layers_{tier}.png"));
        support::write_png(&path, w, h, &sheet);
        eprintln!("kick layers: {}", path.display());
    }
}
