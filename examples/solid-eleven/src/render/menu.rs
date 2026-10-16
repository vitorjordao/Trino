//! The menus' 3D backdrops (after the reference art): the stadium at night
//! with a huddle of celebrating players behind the title, and a player on
//! a glowing pedestal for the squad, editor and market screens.

use super::anim::{AnimState, body_matrix, joint_matrices, plant};
use super::clips_gen::JOINTS;
use super::{
    CamState, Detail, Frustum, Look, M_PEDESTAL, Sky, WORLD, draw_body, draw_stadium, unlit,
};
use trino_core::{Color, LightTier, Mat34, Material, Renderer, Vec3};

/// The title screen: night, a low camera on the touchline, five players
/// celebrating in two kits.
pub fn title(r: &mut dyn Renderer, looks: &[Look; 5], anims: &[AnimState], aspect: f32) {
    let hifi = r.caps().light_tier == LightTier::PerPixel;
    r.begin_frame(Sky::Night.clear());
    let cam = CamState {
        focus: Vec3::new(10.0, 2.4, 6.0),
        height: 0.4,
        back: 13.0,
        fov: 0.72,
    };
    r.set_camera(&cam.camera());
    r.set_light(&Sky::Night.light());
    let fr = Frustum::new(&cam, aspect);
    draw_stadium(r, &fr, hifi, Sky::Night.tint());
    // The huddle, facing the camera and each other.
    let spots = [
        (8.6, 8.5, 0.2, 1.0),
        (10.0, 7.4, -0.3, 0.95),
        (11.4, 8.2, -0.7, 0.7),
        (9.4, 6.2, 0.5, 0.86),
        (12.2, 6.8, -0.9, 0.4),
    ];
    let mut j = [Mat34::IDENTITY; JOINTS];
    for (k, &(x, z, fx, fz)) in spots.iter().enumerate() {
        let n = trino_core::math3d::sqrt(fx * fx + fz * fz);
        let body = body_matrix(Vec3::new(x, 0.0, z), fx / n, fz / n, WORLD);
        let detail = if hifi { Detail::Full } else { Detail::Near };
        let pose = anims[k].pose_masked(detail.mask());
        joint_matrices(&body, &pose, &mut j, detail.mask());
        plant(&mut j, detail.mask(), anims[k].grounded(), body.m[1][3]);
        draw_body(r, &body, &j, &looks[k], detail, Sky::Dusk.tint());
    }
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
) {
    let hifi = r.caps().light_tier == LightTier::PerPixel;
    r.begin_frame(Sky::Night.clear());
    // Looking up the pitch from the center spot: the far stand behind.
    // Far enough back for the whole body between the header and the
    // footer.
    let cam = CamState {
        focus: Vec3::new(shift, 0.98, 0.0),
        height: 0.32,
        back: 6.2,
        fov: 0.62,
    };
    r.set_camera(&cam.camera());
    r.set_light(&Sky::Dusk.light());
    let fr = Frustum::new(&cam, aspect);
    draw_stadium(r, &fr, hifi, Sky::Night.tint());
    let (s, c) = (trino_core::math3d::sin(yaw), trino_core::math3d::cos(yaw));
    let base = Vec3::new(0.0, 0.0, 0.0);
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
