//! The menus' 3D backdrops (after the reference art): the stadium at night
//! with the trophy ceremony behind the title and at the end of a season,
//! and a player on a glowing pedestal for the squad, editor and market
//! screens.

use super::anim::{AnimState, body_matrix, joint_matrices, plant};
use super::clips_gen::JOINTS;
use super::{
    CamState, Detail, Frustum, Look, M_PEDESTAL, M_TROPHY, S_CONFETTI, Sky, WORLD, draw_body,
    draw_stadium, plain, unlit,
};
use trino_core::{BillboardParams, Color, Mat34, Material, Renderer, Vec2, Vec3};

/// The trophy ceremony: at night under the floodlights, the captain
/// (`looks[0]`, playing `TrophyLift`) lifts the cup over his head between
/// four teammates celebrating, confetti falling. `cam` frames the group
/// standing around (`at`, on the turf); `time` drives the confetti.
pub fn ceremony(
    r: &mut dyn Renderer,
    looks: &[Look; 5],
    anims: &[AnimState],
    aspect: f32,
    cam: &CamState,
    at: Vec3,
    time: f32,
    retro: bool,
) {
    let hifi = super::hifi(r, retro);
    r.begin_frame(Sky::Night.clear());
    r.set_camera(&cam.camera());
    // The floodlights: the players stand in daylight-white light in front
    // of the night stand.
    r.set_light(&Sky::Day.light());
    super::set_ao(r, hifi);
    let fr = Frustum::new(cam, aspect);
    draw_stadium(r, &fr, hifi, Sky::Night.tint());
    // The captain in front, the others around him facing the camera.
    let spots = [
        (0.0, 0.0, 0.0, 1.0),
        (-1.3, -0.9, 0.35, 1.0),
        (1.3, -0.8, -0.35, 1.0),
        (-2.3, -2.0, 0.5, 0.9),
        (2.4, -1.9, -0.5, 0.9),
    ];
    let detail = if hifi { Detail::Full } else { Detail::Near };
    let mut j = [Mat34::IDENTITY; JOINTS];
    for (k, &(x, z, fx, fz)) in spots.iter().enumerate() {
        let n = trino_core::math3d::sqrt(fx * fx + fz * fz);
        let body = body_matrix(at + Vec3::new(x, 0.0, z), fx / n, fz / n, WORLD);
        let pose = anims[k].pose_masked(detail.mask());
        joint_matrices(&body, &pose, &mut j, detail.mask());
        plant(&mut j, detail.mask(), anims[k].grounded(), body.m[1][3]);
        draw_body(r, &body, &j, &looks[k], detail, Color::WHITE);
        if k == 0 {
            draw_trophy(r, &body, &j, detail);
        }
    }
    confetti(r, at, time, looks);
}

/// The cup between the captain's hands (forearms' ends on the PC, the
/// whole arms' on consoles), upright.
fn draw_trophy(r: &mut dyn Renderer, body: &Mat34, j: &[Mat34; JOINTS], detail: Detail) {
    use super::anim::joint::{ARM_L, ARM_R, FOREARM_L, FOREARM_R};
    let hand = |m: &Mat34, len: f32| m.transform_point(Vec3::new(0.0, -len, 0.0));
    let (l, rr) = if detail == Detail::Full {
        (hand(&j[FOREARM_L], 0.31), hand(&j[FOREARM_R], 0.31))
    } else {
        (hand(&j[ARM_L], 0.6), hand(&j[ARM_R], 0.6))
    };
    let mid = (l + rr) * 0.5;
    // A big cup: 1.3 x the model.
    let mut m = *body;
    for row in m.m.iter_mut() {
        for v in row.iter_mut().take(3) {
            *v *= TROPHY_SCALE;
        }
    }
    m.m[0][3] = mid.x;
    m.m[1][3] = mid.y - 0.02 * WORLD;
    m.m[2][3] = mid.z;
    r.draw_model_matrix(M_TROPHY, &m, Material::VertexLit, &plain(Color::WHITE));
}

const TROPHY_SCALE: f32 = 1.3;

/// Confetti over the group: a fixed scatter of flakes in the kits' colors
/// and gold, falling and drifting (visual only, no RNG).
fn confetti(r: &mut dyn Renderer, at: Vec3, time: f32, looks: &[Look; 5]) {
    let colors = [
        looks[0].shirt,
        looks[0].shorts,
        Color::rgb(250, 210, 70),
        Color::rgb(245, 245, 250),
    ];
    for i in 0..48u32 {
        let h = |k: u32| (i.wrapping_mul(k) >> 8) as f32 / 16_777_216.0;
        let (hx, hz, ph) = (h(2_654_435_761), h(2_246_822_519), h(3_266_489_917));
        let fall = time * 1.4 + ph * 6.0;
        let y = 6.0 - (fall - (fall / 6.0) as i32 as f32 * 6.0);
        let sway = trino_core::math3d::sin(time * 2.0 + ph * core::f32::consts::TAU) * 0.3;
        // Behind the captain (the camera is on +z): a flake never crosses
        // his face.
        let p = at + Vec3::new((hx - 0.5) * 5.0 + sway, y, -0.4 - hz * 3.0);
        r.draw_billboard(
            S_CONFETTI,
            p * WORLD,
            &BillboardParams {
                size: Vec2::new(0.09 * WORLD, 0.06 * WORLD),
                tint: colors[i as usize % colors.len()],
                ..BillboardParams::DEFAULT
            },
        );
    }
}

/// The title screen's backdrop: the ceremony on the right of the menu.
pub fn title(
    r: &mut dyn Renderer,
    looks: &[Look; 5],
    anims: &[AnimState],
    aspect: f32,
    time: f32,
    retro: bool,
) {
    // The group stands right of the menu, over the club card.
    let at = Vec3::new(10.0, 0.0, 7.0);
    // (Screen right is -x from this side of the pitch.) The eye stays
    // high enough that the turf under the frame's bottom edge is past the
    // near plane (3 m at the drawing scale).
    let cam = CamState {
        focus: Vec3::new(10.0 + 3.3 * aspect / (16.0 / 9.0), 0.75, 7.0),
        height: 1.4,
        back: 9.0,
        fov: 0.72,
    };
    ceremony(r, looks, anims, aspect, &cam, at, time, retro);
}

/// A player on the pedestal, turning slowly, the stadium behind him.
pub fn pedestal(
    r: &mut dyn Renderer,
    look: &Look,
    anim: &AnimState,
    yaw: f32,
    glow: Color,
    aspect: f32,
    shift: f32,
    retro: bool,
) {
    let hifi = super::hifi(r, retro);
    r.begin_frame(Sky::Night.clear());
    // Looking up the pitch from beside the center spot (on it, the halfway
    // line ran up behind the player as if out of his shorts): the far
    // stand behind. The body and the pedestal sit above the screens'
    // stat panels; from 11 m back with a narrow lens, so the turf nearest
    // the lens (4 m off) stays past the near plane (3.2 m at 1/64 scale).
    let spot = Vec3::new(3.5, 0.0, 0.0);
    let cam = CamState {
        focus: spot + Vec3::new(shift, 0.46, 0.0),
        height: 1.15,
        back: 10.9,
        fov: 0.5,
    };
    r.set_camera(&cam.camera());
    r.set_light(&Sky::Dusk.light());
    super::set_ao(r, hifi);
    let fr = Frustum::new(&cam, aspect);
    draw_stadium(r, &fr, hifi, Sky::Night.tint());
    let (s, c) = (trino_core::math3d::sin(yaw), trino_core::math3d::cos(yaw));
    let base = spot;
    r.draw_model_matrix(
        super::M_PEDESTAL,
        &super::place(base - Vec3::new(0.0, 0.12, 0.0), 0.0, 1.0, 1.0),
        Material::VertexLit,
        &unlit(glow),
    );
    let body = body_matrix(base, s, c, WORLD);
    let mut j = [Mat34::IDENTITY; JOINTS];
    let detail = if hifi { Detail::Full } else { Detail::Near };
    let pose = anim.pose_masked(detail.mask());
    joint_matrices(&body, &pose, &mut j, detail.mask());
    plant(&mut j, detail.mask(), anim.grounded(), body.m[1][3]);
    draw_body(r, &body, &j, look, detail, Color::WHITE);
    let _ = M_PEDESTAL;
}
