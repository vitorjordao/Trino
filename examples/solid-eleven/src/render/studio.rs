//! One posed figure on a patch of turf, lit like the match: the pose
//! sheets (`xtask/tests/solid_eleven_poses.rs`) look at every clip frame
//! by frame from several sides with it.

use super::anim::{Pose, body_matrix, joint_matrices, plant};
use super::clips_gen::JOINTS;
use super::{CamState, Detail, Look, M_PITCH_FINE, Sky, WORLD, draw_body, place, plain};
use trino_core::{Color, Mat34, Material, Renderer, Vec3};

/// Draw `pose` on a body at the center spot facing `face` (a unit ground
/// vector), the camera `cam` = (height over a 0.9 m focus, distance back)
/// in meters. Keep it 6 m or more away: the world is drawn at 1/64 and the
/// near plane cuts closer bodies.
pub fn figure(
    r: &mut dyn Renderer,
    pose: &Pose,
    look: &Look,
    face: (f32, f32),
    detail: Detail,
    cam: (f32, f32),
    snap: bool,
) {
    let framing = Framing {
        focus_y: 1.05,
        fov: 0.32,
    };
    figure_framed(r, pose, look, face, detail, cam, snap, framing);
}

/// Where [`figure_framed`]'s camera looks: the focus height (m) and the
/// field of view (radians).
#[derive(Clone, Copy, Debug)]
pub struct Framing {
    pub focus_y: f32,
    pub fov: f32,
}

/// [`figure`] with its own framing: a head close-up looks at 1.7 m with a
/// narrow field of view from the same distance.
#[allow(clippy::too_many_arguments)]
pub fn figure_framed(
    r: &mut dyn Renderer,
    pose: &Pose,
    look: &Look,
    face: (f32, f32),
    detail: Detail,
    cam: (f32, f32),
    snap: bool,
    framing: Framing,
) {
    r.begin_frame(Color::rgb(70, 78, 96));
    let cs = CamState {
        focus: Vec3::new(0.0, framing.focus_y, 0.0),
        height: cam.0,
        back: cam.1,
        fov: framing.fov,
        wide: false,
    };
    r.set_camera(&cs.camera());
    r.set_light(&Sky::Day.light());
    // The four fine tiles around the center spot.
    for (ix, iz) in [(5usize, 3usize), (5, 4), (6, 3), (6, 4)] {
        r.draw_model_matrix(
            M_PITCH_FINE[ix * 8 + iz],
            &place(Vec3::ZERO, 0.0, 1.0, 1.0),
            Material::VertexLit,
            &plain(Color::WHITE),
        );
    }
    let body = body_matrix(Vec3::ZERO, face.0, face.1, WORLD);
    let mut j = [Mat34::IDENTITY; JOINTS];
    joint_matrices(&body, pose, &mut j, detail.mask());
    plant(&mut j, detail.mask(), snap, body.m[1][3]);
    draw_body(r, &body, &j, look, detail, Color::WHITE);
}
