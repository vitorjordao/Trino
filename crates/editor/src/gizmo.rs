//! Orbit camera, projection helpers (the same math as
//! `trino_core::render3d::tessellate`, so overlays line up with rendered
//! pixels) and the transform gizmo (move / rotate / scale).

use eframe::egui::{self, Color32, Pos2, Rect, Stroke, Vec2};
use trino_core::{Camera3, Mat34, Vec3};
use trino_scene::Transform;

use crate::theme::c;

// --------------------------------------------------------------- camera --

/// Editor orbit camera with critically-damped smoothing (every move eases).
#[derive(Clone, Copy, Debug)]
pub struct OrbitCam {
    pub target: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    pub dist: f32,
    pub fov: f32,
    /// Near-orthographic projection (Numpad 5 / the axis widget's centre):
    /// see [`OrbitCam::camera`].
    pub ortho: bool,
    /// Fly mode (RMB + WASD/QE) speed, world units per second.
    pub fly_speed: f32,
    goal_target: Vec3,
    goal_yaw: f32,
    goal_pitch: f32,
    goal_dist: f32,
}

/// Pitch limit: just short of the poles, where `look_at` has no up.
pub const MAX_PITCH: f32 = 1.54;
/// Radians of orbit per pixel dragged (the view and the axis widget alike).
const ORBIT_PER_PX: f32 = 0.008;
/// Fly mode: Shift multiplies the speed by this.
pub const FLY_BOOST: f32 = 2.5;
/// Near-orthographic mode: the eye backs off to at least this far (world
/// units), and at least `ORTHO_FACTOR` times the orbit distance.
const ORTHO_MIN_PULL: f32 = 2000.0;
const ORTHO_FACTOR: f32 = 100.0;

/// What the renderer draws the Scene view with. In perspective, the camera
/// itself. The engine has no orthographic projection (the consoles don't
/// need one), so the near-orthographic mode puts the eye very far back with
/// a field of view so narrow that the view at the target keeps its size.
/// At that distance a 32-bit depth buffer of `1 - NEAR/z` would resolve
/// only metres, so the world is drawn scaled down around the target
/// (`world`, applied to every model matrix): the eye is 1 unit away and
/// depth keeps millimetres. The picture is the same (a perspective is
/// scale-invariant); [`OrbitCam::camera`] is the unscaled equivalent the
/// overlays and picking use.
#[derive(Clone, Copy, Debug)]
pub struct RenderCam {
    pub camera: Camera3,
    /// World to render space (identity in perspective).
    pub world: Mat34,
    /// Render units per world unit (1 in perspective).
    pub scale: f32,
    /// How much farther the eye is than the orbit distance (0 in
    /// perspective): distance-based effects (fog) shift by it.
    pub pull: f32,
}

impl RenderCam {
    /// A camera drawn as is (perspective, unscaled world).
    pub fn plain(camera: Camera3) -> Self {
        RenderCam {
            camera,
            world: Mat34::IDENTITY,
            scale: 1.0,
            pull: 0.0,
        }
    }

    /// A fog range (world units from the orbit eye) in render space.
    pub fn fog(&self, fog: trino_core::Fog) -> trino_core::Fog {
        if fog.far <= fog.near {
            return fog;
        }
        trino_core::Fog {
            near: (fog.near + self.pull) * self.scale,
            far: (fog.far + self.pull) * self.scale,
            ..fog
        }
    }
}

impl Default for OrbitCam {
    fn default() -> Self {
        let mut cam = OrbitCam {
            target: Vec3::new(0.0, 0.5, 0.0),
            yaw: 0.6,
            pitch: 0.45,
            dist: 12.0,
            fov: 55f32.to_radians(),
            ortho: false,
            fly_speed: 6.0,
            goal_target: Vec3::ZERO,
            goal_yaw: 0.0,
            goal_pitch: 0.0,
            goal_dist: 0.0,
        };
        cam.snap_goals();
        cam
    }
}

/// The six views of the axis widget: `k < 3` looks from the +k side
/// (X right, Y top, Z back), `k + 3` from the -k side. Yaw and pitch of the
/// orbit eye; the top and bottom views keep the current yaw.
pub fn axis_view(idx: usize, yaw: f32) -> (f32, f32) {
    use std::f32::consts::{FRAC_PI_2, PI};
    match idx {
        0 => (FRAC_PI_2, 0.0),
        3 => (-FRAC_PI_2, 0.0),
        1 => (yaw, MAX_PITCH),
        4 => (yaw, -MAX_PITCH),
        2 => (PI, 0.0),
        _ => (0.0, 0.0),
    }
}

/// Which of the six [`axis_view`]s a yaw/pitch looks from, if any.
pub fn aligned_axis(yaw: f32, pitch: f32) -> Option<usize> {
    use std::f32::consts::{FRAC_PI_2, PI, TAU};
    const EPS: f32 = 0.01;
    if pitch >= MAX_PITCH - EPS {
        return Some(1);
    }
    if pitch <= -MAX_PITCH + EPS {
        return Some(4);
    }
    if pitch.abs() > EPS {
        return None;
    }
    let y = yaw.rem_euclid(TAU);
    let near = |a: f32| (y - a).abs() < EPS || (y - a - TAU).abs() < EPS;
    [(0, FRAC_PI_2), (3, 3.0 * FRAC_PI_2), (2, PI), (5, 0.0)]
        .into_iter()
        .find(|&(_, a)| near(a))
        .map(|(k, _)| k)
}

impl OrbitCam {
    fn snap_goals(&mut self) {
        self.goal_target = self.target;
        self.goal_yaw = self.yaw;
        self.goal_pitch = self.pitch;
        self.goal_dist = self.dist;
    }

    fn offset(yaw: f32, pitch: f32) -> Vec3 {
        Vec3::new(
            pitch.cos() * yaw.sin(),
            pitch.sin(),
            -pitch.cos() * yaw.cos(),
        )
    }

    /// The orbit eye: where a perspective view looks from (and what LOD
    /// distances are measured from, in either projection).
    pub fn eye(&self) -> Vec3 {
        self.target + Self::offset(self.yaw, self.pitch) * self.dist
    }

    /// The camera the overlays project with. In the near-orthographic mode
    /// its eye is [`RenderCam::pull`] farther back with a narrow field of
    /// view, so the target's plane keeps its size on screen.
    pub fn camera(&self) -> Camera3 {
        let (dist, fov) = self.projection();
        Camera3 {
            eye: self.target + Self::offset(self.yaw, self.pitch) * dist,
            target: self.target,
            fov_y: fov,
        }
    }

    /// The camera and world transform the renderer draws with.
    pub fn render_cam(&self) -> RenderCam {
        let camera = self.camera();
        if !self.ortho {
            return RenderCam::plain(camera);
        }
        let d = camera.eye - camera.target;
        let len = d.dot(d).sqrt();
        let s = 1.0 / len;
        RenderCam {
            camera: Camera3 {
                eye: d * s,
                target: Vec3::ZERO,
                fov_y: camera.fov_y,
            },
            world: Mat34::from_rotation_scale_translation(
                Vec3::ZERO,
                Vec3::new(s, s, s),
                -self.target * s,
            ),
            scale: s,
            pull: len - self.dist,
        }
    }

    /// Eye distance and vertical field of view of [`camera`](Self::camera).
    fn projection(&self) -> (f32, f32) {
        if !self.ortho {
            return (self.dist, self.fov);
        }
        let pull = (self.dist * ORTHO_FACTOR).max(ORTHO_MIN_PULL);
        // The renderer's focal is the core's cot(fov/2) (an approximation:
        // ~1% off near zero), so solve for the angle whose core cotangent
        // keeps the target plane exactly as big as in perspective.
        let cot = |h: f32| trino_core::math3d::cos(h) / trino_core::math3d::sin(h);
        let want = cot(self.fov * 0.5) * pull / self.dist;
        let mut h = (1.0 / want).atan();
        for _ in 0..3 {
            h *= cot(h) / want;
        }
        (pull, h * 2.0)
    }

    /// Advance smoothing; true while still moving.
    pub fn step(&mut self, dt: f32) -> bool {
        let k = 1.0 - (-dt * 16.0).exp();
        let lerp = |a: f32, b: f32| a + (b - a) * k;
        self.target = self.target + (self.goal_target - self.target) * k;
        self.yaw = lerp(self.yaw, self.goal_yaw);
        self.pitch = lerp(self.pitch, self.goal_pitch);
        self.dist = lerp(self.dist, self.goal_dist);
        let d = self.goal_target - self.target;
        let moving = d.dot(d) > 1e-8
            || (self.goal_yaw - self.yaw).abs() > 1e-4
            || (self.goal_pitch - self.pitch).abs() > 1e-4
            || (self.goal_dist - self.dist).abs() > 1e-4 * self.goal_dist.max(1.0);
        if !moving {
            self.snap_goals();
        }
        moving
    }

    pub fn orbit(&mut self, delta: Vec2) {
        self.goal_yaw += delta.x * ORBIT_PER_PX;
        self.goal_pitch = (self.goal_pitch + delta.y * ORBIT_PER_PX).clamp(-MAX_PITCH, MAX_PITCH);
        // Orbit responds immediately (smoothing only on discrete moves).
        self.yaw = self.goal_yaw;
        self.pitch = self.goal_pitch;
    }

    pub fn pan(&mut self, delta: Vec2, viewport_h: f32) {
        let view = self.camera().view();
        let right = Vec3::new(view.m[0][0], view.m[0][1], view.m[0][2]);
        let up = Vec3::new(view.m[1][0], view.m[1][1], view.m[1][2]);
        let world_per_px = 2.0 * self.dist * (self.fov * 0.5).tan() / viewport_h.max(1.0);
        let shift = (right * -delta.x + up * delta.y) * world_per_px;
        self.goal_target += shift;
        self.target = self.goal_target;
    }

    pub fn zoom(&mut self, scroll: f32) {
        self.goal_dist = (self.goal_dist * (-scroll * 0.0015).exp()).clamp(0.3, 600.0);
    }

    /// Alt + RMB drag: dolly by a vertical drag (up = in), immediately.
    pub fn dolly(&mut self, dy: f32) {
        self.goal_dist = (self.goal_dist * (dy * 0.01).exp()).clamp(0.3, 600.0);
        self.dist = self.goal_dist;
    }

    /// Fly mode mouselook: turn the view around the eye (mouse right turns
    /// right, mouse up looks up); the orbit target moves, the eye stays.
    pub fn look(&mut self, delta: Vec2) {
        self.settle();
        let eye = self.eye();
        self.yaw -= delta.x * ORBIT_PER_PX * 0.5;
        self.pitch = (self.pitch + delta.y * ORBIT_PER_PX * 0.5).clamp(-MAX_PITCH, MAX_PITCH);
        self.target = eye - Self::offset(self.yaw, self.pitch) * self.dist;
        self.snap_goals();
    }

    /// Fly mode movement: `dir` is (right, world up, forward) in -1..=1,
    /// forward along the view vector. Moves eye and target together.
    pub fn fly(&mut self, dir: Vec3, dt: f32, boost: bool) {
        self.settle();
        let forward = -Self::offset(self.yaw, self.pitch);
        let right = Vec3::new(self.yaw.cos(), 0.0, self.yaw.sin());
        let speed = self.fly_speed * if boost { FLY_BOOST } else { 1.0 };
        let step = (right * dir.x + Vec3::new(0.0, dir.y, 0.0) + forward * dir.z) * (speed * dt);
        self.target += step;
        self.snap_goals();
    }

    /// Scroll while flying: change the fly speed (scroll up = faster).
    pub fn fly_speed_scroll(&mut self, scroll: f32) {
        self.fly_speed = (self.fly_speed * (scroll * 0.003).exp()).clamp(0.25, 200.0);
    }

    /// Finish any easing at once (fly mode takes over from the goals).
    fn settle(&mut self) {
        self.target = self.goal_target;
        self.yaw = self.goal_yaw;
        self.pitch = self.goal_pitch;
        self.dist = self.goal_dist;
    }

    /// Ease to frame a sphere. The sphere's centre becomes the orbit centre,
    /// so the next orbit turns around the framed object.
    pub fn focus(&mut self, center: Vec3, radius: f32) {
        self.goal_target = center;
        self.goal_dist = (radius.max(0.25) / (self.fov * 0.5).sin() * 1.3).clamp(0.6, 400.0);
    }

    /// Ease to a canonical view (Blender numpad style).
    pub fn view_from(&mut self, yaw: f32, pitch: f32) {
        // Shortest way around.
        let mut y = yaw;
        while y - self.goal_yaw > std::f32::consts::PI {
            y -= std::f32::consts::TAU;
        }
        while self.goal_yaw - y > std::f32::consts::PI {
            y += std::f32::consts::TAU;
        }
        self.goal_yaw = y;
        self.goal_pitch = pitch;
    }

    /// The axis widget's click: look from that axis (an [`axis_view`]
    /// index), or from the opposite side when the view already looks from
    /// it (Blender's flip). Judged on where the camera is heading, so a
    /// second click during the ease flips too. Returns the view chosen.
    pub fn snap_to_axis(&mut self, idx: usize) -> usize {
        let idx = if aligned_axis(self.goal_yaw, self.goal_pitch) == Some(idx) {
            (idx + 3) % 6
        } else {
            idx
        };
        let (yaw, pitch) = axis_view(idx, self.goal_yaw);
        self.view_from(yaw, pitch);
        idx
    }

    /// Match a game camera (eye/target).
    pub fn look_from(&mut self, eye: Vec3, target: Vec3) {
        let d = eye - target;
        let dist = d.dot(d).sqrt().max(0.3);
        self.goal_target = target;
        self.goal_dist = dist;
        self.goal_pitch = (d.y / dist).clamp(-1.0, 1.0).asin();
        self.goal_yaw = d.x.atan2(-d.z);
    }
}

// ----------------------------------------------------------- projection --

/// Maps world points to panel pixels for one rendered image.
#[derive(Clone, Copy)]
pub struct Proj {
    pub view: Mat34,
    pub eye: Vec3,
    focal: f32,
    half: Vec2,
    /// Where the image is drawn in the panel.
    pub rect: Rect,
    /// Panel pixels per internal pixel.
    scale: f32,
}

const NEAR: f32 = 0.05;

impl Proj {
    pub fn new(cam: &Camera3, internal: (u32, u32), rect: Rect) -> Self {
        let half_fov = cam.fov_y * 0.5;
        // Same focal as tessellate (core sin/cos for bit-level agreement).
        let focal = trino_core::math3d::cos(half_fov) / trino_core::math3d::sin(half_fov)
            * internal.1 as f32
            * 0.5;
        Proj {
            view: cam.view(),
            eye: cam.eye,
            focal,
            half: Vec2::new(internal.0 as f32 * 0.5, internal.1 as f32 * 0.5),
            rect,
            scale: rect.width() / internal.0 as f32,
        }
    }

    pub fn depth(&self, p: Vec3) -> f32 {
        self.view.transform_point(p).z
    }

    pub fn project(&self, p: Vec3) -> Option<Pos2> {
        let v = self.view.transform_point(p);
        if v.z <= NEAR {
            return None;
        }
        let ix = self.half.x + v.x * self.focal / v.z;
        let iy = self.half.y - v.y * self.focal / v.z;
        Some(self.rect.min + Vec2::new(ix, iy) * self.scale)
    }

    /// World-space ray through a panel position.
    pub fn ray(&self, pos: Pos2) -> (Vec3, Vec3) {
        let i = (pos - self.rect.min) / self.scale;
        let dx = (i.x - self.half.x) / self.focal;
        let dy = -(i.y - self.half.y) / self.focal;
        let m = &self.view.m;
        let r = Vec3::new(m[0][0], m[0][1], m[0][2]);
        let u = Vec3::new(m[1][0], m[1][1], m[1][2]);
        let f = Vec3::new(m[2][0], m[2][1], m[2][2]);
        (self.eye, (r * dx + u * dy + f).normalized())
    }

    pub fn forward(&self) -> Vec3 {
        let m = &self.view.m;
        Vec3::new(m[2][0], m[2][1], m[2][2])
    }

    /// World length that spans `px` panel pixels at point `p`.
    pub fn world_per_px(&self, p: Vec3, px: f32) -> f32 {
        let z = self.depth(p).max(NEAR);
        px / self.scale * z / self.focal
    }

    /// Ray ∩ horizontal plane y = h (None when parallel / behind).
    pub fn ground_point(&self, pos: Pos2, h: f32) -> Option<Vec3> {
        self.plane_point(pos, Vec3::new(0.0, h, 0.0), Vec3::new(0.0, 1.0, 0.0))
    }

    /// Ray ∩ the plane through `at` with normal `n` (None when parallel /
    /// behind).
    pub fn plane_point(&self, pos: Pos2, at: Vec3, n: Vec3) -> Option<Vec3> {
        let (o, d) = self.ray(pos);
        let den = d.dot(n);
        if den.abs() < 1e-5 {
            return None;
        }
        let t = (at - o).dot(n) / den;
        (t > 0.0).then(|| o + d * t)
    }
}

// ----------------------------------------------------------- matrices --

pub fn translation(m: &Mat34) -> Vec3 {
    Vec3::new(m.m[0][3], m.m[1][3], m.m[2][3])
}

/// General affine inverse.
pub fn inverse(m: &Mat34) -> Mat34 {
    let a = &m.m;
    let det = a[0][0] * (a[1][1] * a[2][2] - a[1][2] * a[2][1])
        - a[0][1] * (a[1][0] * a[2][2] - a[1][2] * a[2][0])
        + a[0][2] * (a[1][0] * a[2][1] - a[1][1] * a[2][0]);
    let id = if det.abs() > 1e-20 { 1.0 / det } else { 0.0 };
    let mut r = [[0.0f32; 4]; 3];
    r[0][0] = (a[1][1] * a[2][2] - a[1][2] * a[2][1]) * id;
    r[0][1] = (a[0][2] * a[2][1] - a[0][1] * a[2][2]) * id;
    r[0][2] = (a[0][1] * a[1][2] - a[0][2] * a[1][1]) * id;
    r[1][0] = (a[1][2] * a[2][0] - a[1][0] * a[2][2]) * id;
    r[1][1] = (a[0][0] * a[2][2] - a[0][2] * a[2][0]) * id;
    r[1][2] = (a[0][2] * a[1][0] - a[0][0] * a[1][2]) * id;
    r[2][0] = (a[1][0] * a[2][1] - a[1][1] * a[2][0]) * id;
    r[2][1] = (a[0][1] * a[2][0] - a[0][0] * a[2][1]) * id;
    r[2][2] = (a[0][0] * a[1][1] - a[0][1] * a[1][0]) * id;
    for row in &mut r {
        row[3] = -(row[0] * a[0][3] + row[1] * a[1][3] + row[2] * a[2][3]);
    }
    Mat34 { m: r }
}

/// Ray vs oriented box (`min..max` in the space of `world`). Returns the
/// distance along the ray.
pub fn ray_obb(o: Vec3, d: Vec3, world: &Mat34, min: [f32; 3], max: [f32; 3]) -> Option<f32> {
    let inv = inverse(world);
    let lo = inv.transform_point(o);
    let ld = inv.transform_dir(d);
    let (mut t0, mut t1) = (0.0f32, f32::MAX);
    for (k, (o, d)) in [(lo.x, ld.x), (lo.y, ld.y), (lo.z, ld.z)]
        .into_iter()
        .enumerate()
    {
        // Thin boxes (planes) get a little thickness to stay clickable.
        let (mn, mx) = if max[k] - min[k] < 0.02 {
            (min[k] - 0.01, max[k] + 0.01)
        } else {
            (min[k], max[k])
        };
        if d.abs() < 1e-9 {
            if o < mn || o > mx {
                return None;
            }
        } else {
            let (a, b) = ((mn - o) / d, (mx - o) / d);
            let (a, b) = if a < b { (a, b) } else { (b, a) };
            t0 = t0.max(a);
            t1 = t1.min(b);
            if t0 > t1 {
                return None;
            }
        }
    }
    // Local t equals world t only without scale; recompute in world.
    let hit_local = lo + ld * t0;
    let hit = world.transform_point(hit_local);
    let dv = hit - o;
    Some(dv.dot(dv).sqrt())
}

/// The 12 edges of a box, projected (for wireframe overlays).
pub fn box_lines(proj: &Proj, world: &Mat34, min: [f32; 3], max: [f32; 3]) -> Vec<[Pos2; 2]> {
    let corner = |i: usize| {
        Vec3::new(
            if i & 1 == 0 { min[0] } else { max[0] },
            if i & 2 == 0 { min[1] } else { max[1] },
            if i & 4 == 0 { min[2] } else { max[2] },
        )
    };
    let pts: Vec<Option<Pos2>> = (0..8)
        .map(|i| proj.project(world.transform_point(corner(i))))
        .collect();
    let mut out = Vec::new();
    for a in 0..8usize {
        for bit in [1usize, 2, 4] {
            let b = a | bit;
            if b != a
                && let (Some(p), Some(q)) = (pts[a], pts[b])
            {
                out.push([p, q]);
            }
        }
    }
    out
}

// ---------------------------------------------------------------- gizmo --

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum GizmoMode {
    Translate,
    Rotate,
    Scale,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Handle {
    Axis(usize),
    /// Move on the plane normal to this axis (1 = the ground, XZ).
    Plane(usize),
    Ring(usize),
    /// The outer ring: rotate around the view axis.
    ViewRing,
    Uniform,
}

const AXES: [Vec3; 3] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
];
const AXIS_COLORS: [Color32; 3] = [c::AXIS_X, c::AXIS_Y, c::AXIS_Z];
const GIZMO_PX: f32 = 90.0;
/// Screen radius of the view ring, past the axis rings (0.85 × the axis).
const VIEW_RING_PX: f32 = GIZMO_PX * 1.05;
/// Snap steps: move (world units), rotate (degrees), scale.
pub const SNAP_MOVE: f32 = 0.25;
pub const SNAP_DEG: f32 = 15.0;
pub const SNAP_SCALE: f32 = 0.05;

/// The two axes spanning the plane normal to `axis` (also the ring's plane).
fn plane_axes(axis: usize) -> (Vec3, Vec3) {
    match axis {
        0 => (AXES[1], AXES[2]),
        1 => (AXES[2], AXES[0]),
        _ => (AXES[0], AXES[1]),
    }
}

pub struct Gizmo {
    pub origin: Vec3,
    /// World length of an axis handle.
    pub len: f32,
}

impl Gizmo {
    pub fn new(proj: &Proj, origin: Vec3) -> Self {
        Gizmo {
            origin,
            len: proj.world_per_px(origin, GIZMO_PX),
        }
    }

    fn ring(&self, proj: &Proj, axis: usize) -> Vec<Pos2> {
        let (u, v) = plane_axes(axis);
        let r = self.len * 0.85;
        (0..=64)
            .filter_map(|i| {
                let a = i as f32 / 64.0 * std::f32::consts::TAU;
                proj.project(self.origin + (u * a.cos() + v * a.sin()) * r)
            })
            .collect()
    }

    pub fn hit(&self, proj: &Proj, mode: GizmoMode, mouse: Pos2) -> Option<Handle> {
        let o = proj.project(self.origin)?;
        let near_seg = |a: Pos2, b: Pos2| dist_to_segment(mouse, a, b) < 7.0;
        match mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                if mode == GizmoMode::Translate {
                    // The plane that faces the camera most wins an overlap.
                    let fwd = proj.forward();
                    let mut planes: Vec<usize> = (0..3).collect();
                    planes.sort_by(|&a, &b| {
                        AXES[b]
                            .dot(fwd)
                            .abs()
                            .partial_cmp(&AXES[a].dot(fwd).abs())
                            .unwrap()
                    });
                    if let Some(k) = planes.into_iter().find(|&k| {
                        let q = self.plane_quad(proj, k);
                        q.len() == 4 && point_in_quad(mouse, &q)
                    }) {
                        return Some(Handle::Plane(k));
                    }
                } else if (mouse - o).length() < 9.0 {
                    return Some(Handle::Uniform);
                }
                (0..3)
                    .find(|&k| {
                        proj.project(self.origin + AXES[k] * self.len)
                            .is_some_and(|tip| near_seg(o, tip) && (mouse - o).length() > 8.0)
                    })
                    .map(Handle::Axis)
            }
            GizmoMode::Rotate => (0..3)
                .find(|&k| self.ring(proj, k).windows(2).any(|w| near_seg(w[0], w[1])))
                .map(Handle::Ring)
                .or_else(|| {
                    ((mouse - o).length() - VIEW_RING_PX)
                        .abs()
                        .lt(&6.0)
                        .then_some(Handle::ViewRing)
                }),
        }
    }

    /// The small square of a planar move handle, between the two axes
    /// spanning the plane normal to `axis`.
    fn plane_quad(&self, proj: &Proj, axis: usize) -> Vec<Pos2> {
        let (u, v) = plane_axes(axis);
        let (a, b) = (self.len * 0.22, self.len * 0.42);
        [(a, a), (b, a), (b, b), (a, b)]
            .iter()
            .filter_map(|&(x, y)| proj.project(self.origin + u * x + v * y))
            .collect()
    }

    pub fn draw(&self, painter: &egui::Painter, proj: &Proj, mode: GizmoMode, hot: Option<Handle>) {
        let Some(o) = proj.project(self.origin) else {
            return;
        };
        let lit = |h: Handle, base: Color32| {
            if hot == Some(h) {
                Color32::from_rgb(255, 236, 140)
            } else {
                base
            }
        };
        match mode {
            GizmoMode::Translate | GizmoMode::Scale => {
                if mode == GizmoMode::Translate {
                    // Planar handles, tinted like their normal (Unity's
                    // convention: the XZ square is green).
                    for (k, axis_color) in AXIS_COLORS.iter().enumerate() {
                        let q = self.plane_quad(proj, k);
                        if q.len() == 4 {
                            let col = lit(Handle::Plane(k), *axis_color);
                            painter.add(egui::Shape::convex_polygon(
                                q,
                                col.gamma_multiply(0.25),
                                Stroke::new(1.0, col),
                            ));
                        }
                    }
                } else {
                    let col = lit(Handle::Uniform, Color32::WHITE);
                    painter.circle(o, 6.0, col.gamma_multiply(0.3), Stroke::new(1.5, col));
                }
                // Back-to-front so the nearer axis paints last.
                let mut order: Vec<usize> = (0..3).collect();
                order.sort_by(|&a, &b| {
                    proj.depth(self.origin + AXES[b] * self.len)
                        .partial_cmp(&proj.depth(self.origin + AXES[a] * self.len))
                        .unwrap()
                });
                for k in order {
                    let Some(tip) = proj.project(self.origin + AXES[k] * self.len) else {
                        continue;
                    };
                    let col = lit(Handle::Axis(k), AXIS_COLORS[k]);
                    painter.line_segment([o, tip], Stroke::new(2.5, col));
                    if mode == GizmoMode::Translate {
                        let dir = (tip - o).normalized();
                        let n = Vec2::new(-dir.y, dir.x);
                        painter.add(egui::Shape::convex_polygon(
                            vec![tip + dir * 12.0, tip + n * 5.5, tip - n * 5.5],
                            col,
                            Stroke::NONE,
                        ));
                    } else {
                        painter.rect_filled(
                            Rect::from_center_size(tip, Vec2::splat(9.0)),
                            2.0,
                            col,
                        );
                    }
                }
            }
            GizmoMode::Rotate => {
                painter.circle_stroke(o, 3.0, Stroke::new(1.0, Color32::WHITE));
                let col = lit(Handle::ViewRing, Color32::from_white_alpha(170));
                let w = if hot == Some(Handle::ViewRing) {
                    2.5
                } else {
                    1.5
                };
                painter.circle_stroke(o, VIEW_RING_PX, Stroke::new(w, col));
                for (k, axis_color) in AXIS_COLORS.iter().enumerate() {
                    let pts = self.ring(proj, k);
                    let col = lit(Handle::Ring(k), *axis_color);
                    let w = if hot == Some(Handle::Ring(k)) {
                        3.0
                    } else {
                        2.0
                    };
                    painter.add(egui::Shape::line(pts, Stroke::new(w, col)));
                }
            }
        }
    }
}

/// An in-progress gizmo drag.
pub struct Drag {
    pub handle: Handle,
    start_mouse: Pos2,
    start: Transform,
    /// Parent world matrix (entity world = parent * local).
    parent: Mat34,
    origin: Vec3,
    origin_px: Pos2,
    axis_px: Vec2,
    px_per_unit: f32,
    plane_start: Option<Vec3>,
    /// The view axis when the drag began (the view ring's axis).
    view_axis: Vec3,
    /// Axis handles are shared by move and scale: which one this drag is.
    translate: bool,
    len: f32,
}

impl Drag {
    pub fn begin(
        mode: GizmoMode,
        handle: Handle,
        gizmo: &Gizmo,
        proj: &Proj,
        mouse: Pos2,
        world: &Mat34,
        local: Transform,
    ) -> Option<Drag> {
        let parent = world.mul(&inverse(&local.matrix()));
        let origin_px = proj.project(gizmo.origin)?;
        let (axis_px, px_per_unit) = match handle {
            Handle::Axis(k) => {
                let tip = proj.project(gizmo.origin + AXES[k] * gizmo.len)?;
                let v = tip - origin_px;
                (v.normalized(), v.length() / gizmo.len)
            }
            _ => (Vec2::ZERO, 1.0),
        };
        let plane_start = match handle {
            Handle::Plane(k) => proj.plane_point(mouse, gizmo.origin, AXES[k]),
            _ => None,
        };
        Some(Drag {
            handle,
            start_mouse: mouse,
            start: local,
            parent,
            origin: gizmo.origin,
            origin_px,
            axis_px,
            px_per_unit: px_per_unit.max(1e-6),
            plane_start,
            view_axis: proj.forward(),
            translate: mode == GizmoMode::Translate,
            len: gizmo.len,
        })
    }

    /// Distance moved along the dragged axis (world units, snapped).
    fn axis_amount(&self, mouse: Pos2, snap: bool) -> f32 {
        let s = (mouse - self.start_mouse).dot(self.axis_px) / self.px_per_unit;
        if snap {
            (s / SNAP_MOVE).round() * SNAP_MOVE
        } else {
            s
        }
    }

    /// World offset on the dragged plane (snapped per world axis).
    fn plane_amount(&self, proj: &Proj, mouse: Pos2, snap: bool) -> Option<Vec3> {
        let Handle::Plane(k) = self.handle else {
            return None;
        };
        let (a, b) = (
            self.plane_start?,
            proj.plane_point(mouse, self.origin, AXES[k])?,
        );
        let snapf = |v: f32| {
            if snap {
                (v / SNAP_MOVE).round() * SNAP_MOVE
            } else {
                v
            }
        };
        let d = b - a;
        let mut d = [snapf(d.x), snapf(d.y), snapf(d.z)];
        d[k] = 0.0;
        Some(Vec3::new(d[0], d[1], d[2]))
    }

    /// Ring drags: (screen angle swept, rotation in degrees), both snapped.
    fn ring_amount(&self, proj: &Proj, mouse: Pos2, snap: bool) -> (f32, f32) {
        let a0 = (self.start_mouse - self.origin_px).angle();
        let a1 = (mouse - self.origin_px).angle();
        let mut screen = a1 - a0;
        // Keep the sweep within ±180° so the readout doesn't jump.
        if screen > std::f32::consts::PI {
            screen -= std::f32::consts::TAU;
        } else if screen < -std::f32::consts::PI {
            screen += std::f32::consts::TAU;
        }
        // Toward-viewer axes rotate CCW on screen for positive angles.
        let axis = match self.handle {
            Handle::Ring(k) => AXES[k],
            _ => self.view_axis,
        };
        let sign = if axis.dot(proj.forward()) < 0.0 {
            -1.0
        } else {
            1.0
        };
        let mut deg = (screen * sign).to_degrees();
        if snap {
            deg = (deg / SNAP_DEG).round() * SNAP_DEG;
            screen = deg.to_radians() * sign;
        }
        (screen, deg)
    }

    /// New local transform for the current mouse position.
    pub fn update(&self, proj: &Proj, mouse: Pos2, snap: bool) -> Transform {
        let mut t = self.start;
        let parent_inv = inverse(&self.parent);
        let snapf = |v: f32, step: f32| if snap { (v / step).round() * step } else { v };
        let moved = |t: &mut Transform, d: Vec3| {
            let local = parent_inv.transform_dir(d);
            t.pos = [
                self.start.pos[0] + local.x,
                self.start.pos[1] + local.y,
                self.start.pos[2] + local.z,
            ];
        };
        match self.handle {
            Handle::Axis(k) if self.translate => {
                moved(&mut t, AXES[k] * self.axis_amount(mouse, snap));
            }
            Handle::Plane(_) => {
                if let Some(d) = self.plane_amount(proj, mouse, snap) {
                    moved(&mut t, d);
                }
            }
            Handle::Axis(k) => {
                // Scale along the local axis k.
                let s = (mouse - self.start_mouse).dot(self.axis_px) / self.px_per_unit;
                let f = (1.0 + s / self.len).max(0.01);
                t.scale[k] = snapf(self.start.scale[k] * f, SNAP_SCALE).max(0.001);
            }
            Handle::Uniform => {
                let d = mouse - self.start_mouse;
                let f = (1.0 + (d.x - d.y) * 0.006).max(0.01);
                for k in 0..3 {
                    t.scale[k] = snapf(self.start.scale[k] * f, SNAP_SCALE).max(0.001);
                }
            }
            Handle::Ring(k) => {
                let (_, deg) = self.ring_amount(proj, mouse, snap);
                t.rot = rotate_euler(self.start.rot, &self.parent, k, deg);
            }
            Handle::ViewRing => {
                let (_, deg) = self.ring_amount(proj, mouse, snap);
                t.rot = rotate_euler_axis(self.start.rot, &self.parent, self.view_axis, deg);
            }
        }
        t
    }

    /// What the drag does, drawn over the gizmo: the angle swept (an arc
    /// with its degrees), the distance or the scale factor, and with
    /// snapping on the notches it snaps to.
    pub fn feedback(&self, painter: &egui::Painter, proj: &Proj, mouse: Pos2, snap: bool) {
        let font = egui::FontId::monospace(11.0);
        let label = |at: Pos2, text: String| {
            let galley = painter.layout_no_wrap(text, font.clone(), Color32::WHITE);
            let rect = Rect::from_min_size(at, galley.size()).expand(3.0);
            painter.rect_filled(rect, 3.0, c::BG0.gamma_multiply(0.85));
            painter.galley(at, galley, Color32::WHITE);
        };
        let tick = Color32::from_white_alpha(150);
        match self.handle {
            Handle::Ring(_) | Handle::ViewRing => {
                let (screen, deg) = self.ring_amount(proj, mouse, snap);
                let r = (self.start_mouse - self.origin_px).length().max(12.0);
                let a0 = (self.start_mouse - self.origin_px).angle();
                let at = |a: f32, r: f32| self.origin_px + Vec2::angled(a) * r;
                if snap {
                    // A notch every snap step around the circle.
                    let n = (360.0 / SNAP_DEG) as usize;
                    for i in 0..n {
                        let a = a0 + (i as f32 * SNAP_DEG).to_radians();
                        painter
                            .line_segment([at(a, r - 4.0), at(a, r + 4.0)], Stroke::new(1.0, tick));
                    }
                }
                let steps = ((screen.abs().to_degrees() / 4.0).ceil() as usize).max(1);
                let mut pie = vec![self.origin_px];
                pie.extend((0..=steps).map(|i| at(a0 + screen * i as f32 / steps as f32, r)));
                // A fan from the centre (convex up to half a turn).
                for w in pie[1..].windows(2) {
                    painter.add(egui::Shape::convex_polygon(
                        vec![self.origin_px, w[0], w[1]],
                        c::ACCENT.gamma_multiply(0.22),
                        Stroke::NONE,
                    ));
                }
                painter.add(egui::Shape::line(pie, Stroke::new(1.5, c::ACCENT_TEXT)));
                label(at(a0 + screen, r + 22.0), format!("{deg:+.1}°"));
            }
            Handle::Axis(k) if self.translate => {
                let amount = self.axis_amount(mouse, snap);
                if snap {
                    let step_px = SNAP_MOVE * self.px_per_unit;
                    let n = Vec2::new(-self.axis_px.y, self.axis_px.x);
                    let reach = 400.0 / step_px.max(1.0);
                    let count = reach.min(80.0) as i32;
                    for i in -count..=count {
                        let p = self.origin_px + self.axis_px * (i as f32 * step_px);
                        let w = if i % 4 == 0 { 5.0 } else { 3.0 };
                        painter.line_segment(
                            [p - n * w, p + n * w],
                            Stroke::new(1.0, AXIS_COLORS[k].gamma_multiply(0.8)),
                        );
                    }
                }
                let at = self.origin_px + self.axis_px * (amount * self.px_per_unit);
                label(
                    at + Vec2::new(10.0, 8.0),
                    format!("{}{amount:+.2}", ["X", "Y", "Z"][k]),
                );
            }
            Handle::Plane(k) => {
                if let Some(d) = self.plane_amount(proj, mouse, snap) {
                    let (u, v) = plane_axes(k);
                    let names = |a: Vec3| {
                        if a.x > 0.0 {
                            "X"
                        } else if a.y > 0.0 {
                            "Y"
                        } else {
                            "Z"
                        }
                    };
                    label(
                        mouse + Vec2::new(14.0, 10.0),
                        format!("{}{:+.2} {}{:+.2}", names(u), d.dot(u), names(v), d.dot(v)),
                    );
                }
            }
            Handle::Axis(_) | Handle::Uniform => {
                let t = self.update(proj, mouse, snap);
                let k = match self.handle {
                    Handle::Axis(k) => k,
                    _ => 0,
                };
                let f = t.scale[k] / self.start.scale[k].max(1e-6);
                label(mouse + Vec2::new(14.0, 10.0), format!("×{f:.2}"));
            }
        }
    }
}

// ------------------------------------------------- exact rotation math --
//
// The engine core uses fast approximate trig (deterministic across
// consoles); the editor must not round-trip authored angles through it or a
// pure Y spin would leak ~0.2° into X/Z. Gizmo rotation therefore runs in
// std f32 with Euler continuity, then rounds to 0.001°.

type M3 = [[f32; 3]; 3];

fn m3_mul(a: &M3, b: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| (0..3).map(|k| a[i][k] * b[k][j]).sum()))
}

fn m3_t(a: &M3) -> M3 {
    std::array::from_fn(|i| std::array::from_fn(|j| a[j][i]))
}

/// R = Rz * Ry * Rx (the engine's Euler XYZ convention), degrees.
fn euler_m3(deg: [f32; 3]) -> M3 {
    let [x, y, z] = deg.map(f32::to_radians);
    let (sx, cx) = x.sin_cos();
    let (sy, cy) = y.sin_cos();
    let (sz, cz) = z.sin_cos();
    [
        [cz * cy, cz * sy * sx - sz * cx, cz * sy * cx + sz * sx],
        [sz * cy, sz * sy * sx + cz * cx, sz * sy * cx - cz * sx],
        [-sy, cy * sx, cy * cx],
    ]
}

/// `angle` shifted by multiples of 360° to land nearest `reference`.
fn near(angle: f32, reference: f32) -> f32 {
    angle + ((reference - angle) / 360.0).round() * 360.0
}

/// Euler XYZ (degrees) of a rotation matrix, choosing between the two
/// equivalent solutions the one closest to `reference` (continuity: a Y
/// spin past 90° stays a Y spin instead of flipping X and Z by 180°).
fn m3_euler(r: &M3, reference: [f32; 3]) -> [f32; 3] {
    let sy = (-r[2][0]).clamp(-1.0, 1.0);
    let y = sy.asin();
    let (x, z) = if sy.abs() < 0.99999 {
        (r[2][1].atan2(r[2][2]), r[1][0].atan2(r[0][0]))
    } else {
        // Gimbal lock (y = ±90°): only x ∓ z is defined. Keep the reference
        // X and solve Z. With R = Rz·Ry·Rx and sy = 1: r01 = sin(x−z),
        // r11 = cos(x−z); with sy = −1: r01 = −sin(x+z), r11 = cos(x+z).
        let x = reference[0].to_radians();
        let z = if sy > 0.0 {
            x - r[0][1].atan2(r[1][1])
        } else {
            (-r[0][1]).atan2(r[1][1]) - x
        };
        (x, z)
    };
    let a = [x.to_degrees(), y.to_degrees(), z.to_degrees()];
    let b = [a[0] + 180.0, 180.0 - a[1], a[2] + 180.0];
    let fit = |s: [f32; 3]| -> ([f32; 3], f32) {
        let v: [f32; 3] = std::array::from_fn(|i| near(s[i], reference[i]));
        let d = (0..3).map(|i| (v[i] - reference[i]).abs()).sum();
        (v, d)
    };
    let (va, da) = fit(a);
    let (vb, db) = fit(b);
    let v = if db < da { vb } else { va };
    v.map(|d| (d * 1000.0).round() / 1000.0)
}

/// Rotate an entity's local Euler rotation by `delta_deg` around world axis
/// `axis`, given its parent's world matrix.
pub fn rotate_euler(start: [f32; 3], parent: &Mat34, axis: usize, delta_deg: f32) -> [f32; 3] {
    let mut e = [0.0f32; 3];
    e[axis] = delta_deg;
    rotate_by(start, parent, &euler_m3(e))
}

/// [`rotate_euler`] around any world axis (the view ring's): Rodrigues'
/// rotation of `delta_deg` around the unit vector `axis`.
pub fn rotate_euler_axis(start: [f32; 3], parent: &Mat34, axis: Vec3, delta_deg: f32) -> [f32; 3] {
    let k = axis.normalized();
    let (s, c) = delta_deg.to_radians().sin_cos();
    let t = 1.0 - c;
    let world_delta = [
        [
            c + k.x * k.x * t,
            k.x * k.y * t - k.z * s,
            k.x * k.z * t + k.y * s,
        ],
        [
            k.y * k.x * t + k.z * s,
            c + k.y * k.y * t,
            k.y * k.z * t - k.x * s,
        ],
        [
            k.z * k.x * t - k.y * s,
            k.z * k.y * t + k.x * s,
            c + k.z * k.z * t,
        ],
    ];
    rotate_by(start, parent, &world_delta)
}

/// `start` (local Euler degrees) turned by the world rotation `world_delta`.
fn rotate_by(start: [f32; 3], parent: &Mat34, world_delta: &M3) -> [f32; 3] {
    // Parent rotation (scale stripped).
    let mut p: M3 = std::array::from_fn(|i| std::array::from_fn(|j| parent.m[i][j]));
    for j in 0..3 {
        let len = (p[0][j] * p[0][j] + p[1][j] * p[1][j] + p[2][j] * p[2][j]).sqrt();
        if len > 1e-9 {
            for row in &mut p {
                row[j] /= len;
            }
        }
    }
    // local' = P^T · Δ · P · local
    let local = m3_mul(
        &m3_t(&p),
        &m3_mul(world_delta, &m3_mul(&p, &euler_m3(start))),
    );
    m3_euler(&local, start)
}

fn dist_to_segment(p: Pos2, a: Pos2, b: Pos2) -> f32 {
    let ab = b - a;
    let t = ((p - a).dot(ab) / ab.length_sq().max(1e-6)).clamp(0.0, 1.0);
    (p - (a + ab * t)).length()
}

fn point_in_quad(p: Pos2, q: &[Pos2]) -> bool {
    let mut sign = 0.0f32;
    for i in 0..q.len() {
        let a = q[i];
        let b = q[(i + 1) % q.len()];
        let cross = (b - a).x * (p - a).y - (b - a).y * (p - a).x;
        if cross.abs() < 1e-6 {
            continue;
        }
        if sign == 0.0 {
            sign = cross.signum();
        } else if cross.signum() != sign {
            return false;
        }
    }
    // A quad seen edge-on has no area: it must not swallow clicks on the
    // line it collapses to (the axis handles lie on that line).
    sign != 0.0
}

// ------------------------------------------------------- axis widget --

/// What the orientation widget asks of the camera.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum AxisEvent {
    /// An axis tip was clicked: look from it ([`OrbitCam::snap_to_axis`],
    /// which flips to the other side when the view already looks from it).
    Snap(usize),
    /// The widget is being dragged: orbit like a drag on the view.
    Orbit(Vec2),
    /// The centre (or the projection label) was clicked.
    ToggleOrtho,
}

/// What a point on the widget lands on.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum AxisHit {
    Tip(usize),
    Center,
    /// The disc around them (dragging there orbits).
    Disc,
}

pub const AXIS_WIDGET_RADIUS: f32 = 34.0;
const AXIS_TIP_LEN: f32 = 24.0;
const AXIS_TIP_R: f32 = 7.0;
const AXIS_CENTER_R: f32 = 5.0;

/// The six tips (an [`axis_view`] index each) for a view matrix, back to
/// front, with their depth (> 0: pointing away from the viewer).
pub fn axis_tips(view: &Mat34, center: Pos2) -> Vec<(usize, Pos2, f32)> {
    let (rx, ry, rz) = (view.m[0], view.m[1], view.m[2]);
    let mut items = Vec::with_capacity(6);
    for k in 0..3usize {
        let v = Vec2::new(rx[k], -ry[k]) * AXIS_TIP_LEN; // screen-space axis
        items.push((k, center + v, rz[k]));
        items.push((k + 3, center - v, -rz[k]));
    }
    items.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    items
}

/// Hit test: the front-most tip under `pos` wins, then the centre, then
/// the disc. Looking down an axis puts both of its tips on the centre, so
/// there a click flips the view; elsewhere it toggles the projection.
pub fn axis_hit(tips: &[(usize, Pos2, f32)], center: Pos2, pos: Pos2) -> Option<AxisHit> {
    if let Some(&(idx, _, _)) = tips
        .iter()
        .rev()
        .find(|(_, tip, _)| (pos - *tip).length() <= AXIS_TIP_R + 1.0)
    {
        return Some(AxisHit::Tip(idx));
    }
    let d = (pos - center).length();
    if d <= AXIS_CENTER_R + 2.0 {
        Some(AxisHit::Center)
    } else if d <= AXIS_WIDGET_RADIUS {
        Some(AxisHit::Disc)
    } else {
        None
    }
}

/// The XYZ orientation widget (top-right of the scene view), Blender-style:
/// click a tip to look from that axis (again to look from the other side),
/// drag anywhere on it to orbit, click the centre (or the label below) to
/// switch between perspective and orthographic.
pub fn axis_widget(ui: &egui::Ui, proj: &Proj, center: Pos2, ortho: bool) -> Option<AxisEvent> {
    let disc = Rect::from_center_size(center, Vec2::splat(AXIS_WIDGET_RADIUS * 2.0));
    let resp = ui.interact(
        disc,
        ui.id().with("axis-widget"),
        egui::Sense::click_and_drag(),
    );
    resp.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "View orientation")
    });
    let label_rect = Rect::from_center_size(
        center + Vec2::new(0.0, AXIS_WIDGET_RADIUS + 12.0),
        Vec2::new(46.0, 16.0),
    );
    let name = if ortho { "Ortho" } else { "Persp" };
    let label = ui.interact(
        label_rect,
        ui.id().with("axis-widget-projection"),
        egui::Sense::click(),
    );
    label.widget_info(|| {
        egui::WidgetInfo::labeled(
            egui::WidgetType::Button,
            true,
            format!("Projection: {name} (5)"),
        )
    });
    let label = label.on_hover_text("Perspective / orthographic (5)");

    let tips = axis_tips(&proj.view, center);
    let dragging = resp.dragged_by(egui::PointerButton::Primary);
    let hover = if dragging {
        None
    } else {
        resp.hover_pos().and_then(|p| axis_hit(&tips, center, p))
    };
    if dragging {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
    } else if matches!(hover, Some(AxisHit::Tip(_) | AxisHit::Center)) || label.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }

    let painter = ui.painter();
    let disc_fill = if dragging || hover.is_some() {
        c::BG3.gamma_multiply(0.7)
    } else {
        c::BG0.gamma_multiply(0.55)
    };
    painter.circle_filled(center, AXIS_WIDGET_RADIUS, disc_fill);
    let center_hot = hover == Some(AxisHit::Center) || label.hovered();
    let draw_center = || {
        let (fill, stroke) = if dragging {
            (c::ACCENT, c::ACCENT_TEXT)
        } else if center_hot {
            (c::TEXT, Color32::WHITE)
        } else {
            (c::TEXT_WEAK.gamma_multiply(0.8), c::TEXT_WEAK)
        };
        let r = if center_hot || dragging {
            AXIS_CENTER_R + 1.0
        } else {
            AXIS_CENTER_R
        };
        if ortho {
            // A square centre: the parallel projection.
            painter.rect(
                Rect::from_center_size(center, Vec2::splat(r * 1.8)),
                1.5,
                fill,
                Stroke::new(1.0, stroke),
                egui::StrokeKind::Middle,
            );
        } else {
            painter.circle(center, r, fill, Stroke::new(1.0, stroke));
        }
    };
    let mut center_drawn = false;
    for &(idx, tip, depth) in &tips {
        if depth < 0.0 && !center_drawn {
            draw_center();
            center_drawn = true;
        }
        let k = idx % 3;
        let positive = idx < 3;
        let col = AXIS_COLORS[k];
        if positive {
            painter.line_segment([center, tip], Stroke::new(2.0, col));
        }
        let hovered = hover == Some(AxisHit::Tip(idx));
        let fill = if positive {
            col
        } else {
            col.gamma_multiply(0.35)
        };
        painter.circle(
            tip,
            if hovered {
                AXIS_TIP_R + 1.0
            } else {
                AXIS_TIP_R
            },
            fill,
            Stroke::new(1.0, if hovered { Color32::WHITE } else { col }),
        );
        if positive || hovered {
            painter.text(
                tip,
                egui::Align2::CENTER_CENTER,
                if positive {
                    ["X", "Y", "Z"][k]
                } else {
                    ["-X", "-Y", "-Z"][k]
                },
                egui::FontId::new(if positive { 10.0 } else { 8.5 }, crate::theme::bold()),
                if positive { c::BG0 } else { Color32::WHITE },
            );
        }
    }
    if !center_drawn {
        draw_center();
    }
    painter.rect_filled(
        label_rect,
        4.0,
        if label.hovered() {
            c::BG3.gamma_multiply(0.9)
        } else {
            c::BG0.gamma_multiply(0.55)
        },
    );
    painter.text(
        label_rect.center(),
        egui::Align2::CENTER_CENTER,
        name,
        egui::FontId::proportional(10.5),
        if label.hovered() {
            c::TEXT
        } else {
            c::TEXT_WEAK
        },
    );

    if label.clicked() {
        return Some(AxisEvent::ToggleOrtho);
    }
    if dragging {
        let delta = resp.drag_delta();
        return (delta != Vec2::ZERO).then_some(AxisEvent::Orbit(delta));
    }
    if resp.clicked() {
        return match resp
            .interact_pointer_pos()
            .and_then(|p| axis_hit(&tips, center, p))
        {
            Some(AxisHit::Tip(idx)) => Some(AxisEvent::Snap(idx)),
            Some(AxisHit::Center) => Some(AxisEvent::ToggleOrtho),
            _ => None,
        };
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn snapped_y_spin_stays_exact_and_continuous() {
        // 200° + 15° around Y must read back as (0, 215, 0), not as the
        // equivalent (180, -35, 180), and with no noise on X/Z.
        let r = rotate_euler([0.0, 200.0, 0.0], &Mat34::IDENTITY, 1, 15.0);
        assert_eq!(r, [0.0, 215.0, 0.0]);
        let r = rotate_euler([0.0, 80.0, 0.0], &Mat34::IDENTITY, 1, 30.0);
        assert_eq!(r, [0.0, 110.0, 0.0]);
    }

    #[test]
    fn gimbal_lock_keeps_x_and_reconstructs_the_matrix() {
        for start in [[30.0, 90.0, 10.0], [-20.0, -90.0, 45.0]] {
            let m = euler_m3(start);
            let r = m3_euler(&m, start);
            let back = euler_m3(r);
            for i in 0..3 {
                for j in 0..3 {
                    assert!((m[i][j] - back[i][j]).abs() < 1e-4, "{start:?} -> {r:?}");
                }
            }
        }
    }

    #[test]
    fn rotation_round_trips_through_matrices() {
        for start in [[10.0, 20.0, 30.0], [-45.0, 60.0, 170.0], [0.0, 0.0, 0.0]] {
            let r = m3_euler(&euler_m3(start), start);
            for k in 0..3 {
                assert!((r[k] - start[k]).abs() < 1e-2, "{start:?} -> {r:?}");
            }
        }
    }

    #[test]
    fn projection_and_ray_agree() {
        let cam = Camera3 {
            eye: Vec3::new(3.0, 4.0, -6.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        let proj = Proj::new(
            &cam,
            (640, 360),
            Rect::from_min_size(Pos2::new(10.0, 20.0), Vec2::new(640.0, 360.0)),
        );
        let p = Vec3::new(0.5, 0.25, -0.3);
        let s = proj.project(p).unwrap();
        let (o, d) = proj.ray(s);
        // The point lies on the ray through its own projection.
        let v = p - o;
        let along = v.dot(d);
        let off = v - d * along;
        assert!(off.dot(off).sqrt() < 1e-2, "{off:?}");
    }

    /// A camera on -Z looking at the origin, rendered 1:1 into a 320x240 panel.
    fn front_proj() -> Proj {
        let cam = Camera3 {
            eye: Vec3::new(0.0, 0.0, -10.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        Proj::new(
            &cam,
            (320, 240),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 240.0)),
        )
    }

    fn close(a: Vec3, b: Vec3, eps: f32) -> bool {
        let d = a - b;
        d.dot(d).sqrt() < eps
    }

    #[test]
    fn orbit_camera_moves_clamps_and_settles() {
        let mut cam = OrbitCam::default();
        let eye = cam.eye();
        let d = eye - cam.target;
        assert!(
            (d.dot(d).sqrt() - cam.dist).abs() < 1e-3,
            "eye at orbit distance"
        );
        assert_eq!(cam.camera().target, cam.target);

        // Orbit is immediate, and pitch stops short of the poles.
        cam.orbit(Vec2::new(0.0, 10_000.0));
        assert_eq!(cam.pitch, 1.54);
        cam.orbit(Vec2::new(0.0, -20_000.0));
        assert_eq!(cam.pitch, -1.54);

        // Zoom eases toward a clamped goal; step reports motion until it settles.
        cam.zoom(-1.0e6);
        let mut frames = 0;
        while cam.step(1.0 / 60.0) {
            frames += 1;
            assert!(frames < 1000, "smoothing must converge");
        }
        assert!(frames > 0);
        assert!(
            (cam.dist - 600.0).abs() < 0.1,
            "zoom-out clamps at 600: {}",
            cam.dist
        );
        assert!(!cam.step(1.0 / 60.0), "a settled camera stays still");

        // Focus frames a sphere: farther for a bigger one.
        cam.focus(Vec3::new(1.0, 2.0, 3.0), 1.0);
        while cam.step(0.1) {}
        let small = cam.dist;
        assert!(close(cam.target, Vec3::new(1.0, 2.0, 3.0), 1e-3));
        cam.focus(Vec3::ZERO, 4.0);
        while cam.step(0.1) {}
        assert!(cam.dist > small * 3.0, "{} vs {small}", cam.dist);
    }

    #[test]
    fn orbit_camera_views_take_the_short_way_and_match_a_game_camera() {
        let mut cam = OrbitCam::default();
        cam.orbit(Vec2::new((3.0 - cam.yaw) / 0.008, 0.0));
        assert!((cam.yaw - 3.0).abs() < 1e-4);
        // -3.0 rad is 0.28 rad away going forward through +π, not 6 rad back.
        cam.view_from(-3.0, 0.2);
        while cam.step(0.1) {}
        assert!(
            (cam.yaw - (-3.0 + std::f32::consts::TAU)).abs() < 1e-3,
            "{}",
            cam.yaw
        );
        assert!((cam.pitch - 0.2).abs() < 1e-3);

        let (eye, target) = (Vec3::new(4.0, 3.0, -5.0), Vec3::new(0.0, 1.0, 0.0));
        cam.look_from(eye, target);
        while cam.step(0.1) {}
        assert!(close(cam.eye(), eye, 1e-2), "{:?}", cam.eye());
        assert!(close(cam.target, target, 1e-3));

        // Pan shifts the target sideways in view space, immediately.
        let before = cam.target;
        cam.pan(Vec2::new(100.0, 0.0), 240.0);
        let moved = cam.target - before;
        assert!(moved.dot(moved) > 0.01);
        let fwd = cam.target - cam.eye();
        assert!(moved.dot(fwd).abs() < 1e-2, "pan stays in the view plane");
    }

    /// Eye direction (unit, from the target) of a settled camera.
    fn eye_dir(cam: &OrbitCam) -> Vec3 {
        (cam.eye() - cam.target) * (1.0 / cam.dist)
    }

    #[test]
    fn axis_widget_clicks_look_from_an_axis_then_flip_to_the_other_side() {
        let expect = [
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 1.0, 0.0),
            Vec3::new(0.0, 0.0, 1.0),
            Vec3::new(-1.0, 0.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            Vec3::new(0.0, 0.0, -1.0),
        ];
        for idx in 0..6 {
            let mut cam = OrbitCam::default();
            // First click: look from that axis.
            assert_eq!(cam.snap_to_axis(idx), idx);
            while cam.step(0.1) {}
            assert!(
                close(eye_dir(&cam), expect[idx], 0.05),
                "{idx}: {:?}",
                eye_dir(&cam)
            );
            assert_eq!(aligned_axis(cam.yaw, cam.pitch), Some(idx));
            // Clicking the active axis flips to the opposite one (+X <-> -X).
            let flipped = (idx + 3) % 6;
            assert_eq!(cam.snap_to_axis(idx), flipped);
            while cam.step(0.1) {}
            assert!(close(eye_dir(&cam), expect[flipped], 0.05), "{idx} flipped");
            // And back again.
            assert_eq!(cam.snap_to_axis(flipped), idx);
        }
        // The flip reads where the camera is heading: a double click flips
        // even before the first ease has finished.
        let mut cam = OrbitCam::default();
        cam.snap_to_axis(0);
        cam.step(1.0 / 60.0);
        assert_eq!(cam.snap_to_axis(0), 3);
        // A view off every axis is not aligned; clicking an axis there
        // never flips.
        assert_eq!(aligned_axis(0.6, 0.45), None);
        assert_eq!(aligned_axis(std::f32::consts::FRAC_PI_2, 0.2), None);
        let mut cam = OrbitCam::default();
        assert_eq!(cam.snap_to_axis(2), 2);
        // Yaw is compared modulo a full turn.
        assert_eq!(
            aligned_axis(std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU, 0.0),
            Some(0)
        );
        assert_eq!(aligned_axis(-std::f32::consts::TAU, 0.0), Some(5));
    }

    #[test]
    fn dragging_the_axis_widget_orbits_like_dragging_the_view() {
        let mut cam = OrbitCam::default();
        let (yaw, pitch, target, dist) = (cam.yaw, cam.pitch, cam.target, cam.dist);
        cam.orbit(Vec2::new(50.0, -25.0));
        assert!((cam.yaw - (yaw + 50.0 * ORBIT_PER_PX)).abs() < 1e-6);
        assert!((cam.pitch - (pitch - 25.0 * ORBIT_PER_PX)).abs() < 1e-6);
        // Immediate, around the same target, at the same distance.
        assert!(!cam.step(1.0 / 60.0), "an orbit doesn't ease");
        assert_eq!(cam.target, target);
        assert_eq!(cam.dist, dist);
        // The widget's tips follow the view: after a quarter turn from the
        // front, +X points at the viewer (centre) and +Z to the side.
        let mut cam = OrbitCam::default();
        cam.snap_to_axis(5);
        while cam.step(0.1) {}
        let c = Pos2::new(100.0, 100.0);
        let tip = |cam: &OrbitCam, idx: usize| {
            axis_tips(&cam.camera().view(), c)
                .into_iter()
                .find(|t| t.0 == idx)
                .unwrap()
                .1
        };
        assert!((tip(&cam, 0) - c).x > 20.0, "front view: +X to the right");
        cam.orbit(Vec2::new(std::f32::consts::FRAC_PI_2 / ORBIT_PER_PX, 0.0));
        assert!((tip(&cam, 0) - c).length() < 1.0, "{:?}", tip(&cam, 0));
        assert!((tip(&cam, 2) - c).x.abs() > 20.0);
    }

    #[test]
    fn axis_widget_hit_test_prefers_the_front_tip_then_the_centre() {
        let c = Pos2::new(100.0, 100.0);
        // Off-axis: the centre is free and toggles the projection.
        let cam = OrbitCam::default();
        let tips = axis_tips(&cam.camera().view(), c);
        assert_eq!(axis_hit(&tips, c, c), Some(AxisHit::Center));
        for &(idx, p, _) in &tips {
            assert_eq!(axis_hit(&tips, c, p), Some(AxisHit::Tip(idx)));
        }
        assert_eq!(
            axis_hit(&tips, c, c + Vec2::new(0.0, 31.0)),
            Some(AxisHit::Disc)
        );
        assert_eq!(axis_hit(&tips, c, c + Vec2::new(0.0, 40.0)), None);
        // Looking from +Z: both Z tips sit on the centre and the front one
        // (+Z, toward the viewer) takes the click, so clicking it flips.
        let mut cam = OrbitCam::default();
        cam.snap_to_axis(2);
        while cam.step(0.1) {}
        let tips = axis_tips(&cam.camera().view(), c);
        assert_eq!(axis_hit(&tips, c, c), Some(AxisHit::Tip(2)));
        assert_eq!(cam.snap_to_axis(2), 5);
    }

    #[test]
    fn orthographic_keeps_the_target_plane_and_renders_what_it_projects() {
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 360.0));
        let mut cam = OrbitCam::default();
        let persp = Proj::new(&cam.camera(), (640, 360), rect);
        cam.ortho = true;
        let ortho_cam = cam.camera();
        let ortho = Proj::new(&ortho_cam, (640, 360), rect);
        let d = ortho_cam.eye - ortho_cam.target;
        assert!(d.dot(d).sqrt() >= ORTHO_MIN_PULL - 1.0, "the eye backs off");
        // The plane through the target keeps its size on screen.
        let view = cam.camera().view();
        let right = Vec3::new(view.m[0][0], view.m[0][1], view.m[0][2]);
        let up = Vec3::new(view.m[1][0], view.m[1][1], view.m[1][2]);
        for p in [cam.target, cam.target + right * 3.0, cam.target + up * -2.0] {
            let (a, b) = (persp.project(p).unwrap(), ortho.project(p).unwrap());
            assert!((a - b).length() < 0.5, "{a:?} vs {b:?}");
        }
        // Parallel: a point 5 m behind the target lands (almost) where its
        // front counterpart does; in perspective it moves toward the centre.
        let fwd = ortho.forward();
        let (front, back) = (
            cam.target + right * 3.0,
            cam.target + right * 3.0 + fwd * 5.0,
        );
        let shift = |p: &Proj| (p.project(front).unwrap() - p.project(back).unwrap()).length();
        assert!(shift(&ortho) < 1.0, "{}", shift(&ortho));
        assert!(shift(&persp) > 20.0, "{}", shift(&persp));

        // The renderer's camera over the scaled world projects the same
        // pixels as the overlay camera does over the real one.
        let rc = cam.render_cam();
        let drawn = Proj::new(&rc.camera, (640, 360), rect);
        for p in [cam.target, back, Vec3::new(3.0, -1.0, 7.0)] {
            let (a, b) = (
                ortho.project(p).unwrap(),
                drawn.project(rc.world.transform_point(p)).unwrap(),
            );
            assert!((a - b).length() < 0.5, "{p:?}: {a:?} vs {b:?}");
        }
        // Depth stays well resolved: the target is one render unit away.
        assert!((drawn.depth(rc.world.transform_point(cam.target)) - 1.0).abs() < 1e-3);
        // Fog ranges shift by the pull-back and scale with the world.
        let fog = rc.fog(trino_core::Fog {
            color: trino_core::Color::rgb(0, 0, 0),
            near: 10.0,
            far: 20.0,
        });
        let at = |w: f32| (w + rc.pull) * rc.scale;
        assert!((fog.near - at(10.0)).abs() < 1e-6 && (fog.far - at(20.0)).abs() < 1e-6);
        // Perspective draws as is.
        cam.ortho = false;
        let rc = cam.render_cam();
        assert_eq!(rc.scale, 1.0);
        assert_eq!(rc.camera.eye, cam.eye());
        // Zooming an orthographic view changes what the overlay camera
        // shows (the view's cache key reads it).
        cam.ortho = true;
        let before = cam.camera();
        cam.zoom(500.0);
        while cam.step(0.1) {}
        assert!(cam.camera().fov_y < before.fov_y);
    }

    #[test]
    fn fly_mode_looks_around_the_eye_and_moves_along_the_view() {
        let mut cam = OrbitCam::default();
        let eye = cam.eye();
        let view = cam.camera().view();
        let right = Vec3::new(view.m[0][0], view.m[0][1], view.m[0][2]);
        let fwd = Vec3::new(view.m[2][0], view.m[2][1], view.m[2][2]);
        // Mouselook: the eye stays, mouse right turns right, mouse up looks up.
        cam.look(Vec2::new(40.0, 0.0));
        assert!(close(cam.eye(), eye, 1e-3));
        let new_fwd = (cam.target - cam.eye()) * (1.0 / cam.dist);
        assert!(new_fwd.dot(right) > 0.05, "{new_fwd:?}");
        let before = (cam.target - cam.eye()).y;
        cam.look(Vec2::new(0.0, -40.0));
        assert!((cam.target - cam.eye()).y > before, "looks up");
        assert!(!cam.step(1.0 / 60.0), "fly mode doesn't ease");

        // W: forward along the view vector at fly speed; D: right; E: up.
        let mut cam = OrbitCam::default();
        let (eye, target) = (cam.eye(), cam.target);
        cam.fly(Vec3::new(0.0, 0.0, 1.0), 0.5, false);
        let moved = cam.eye() - eye;
        assert!(close(moved, fwd * (cam.fly_speed * 0.5), 1e-3), "{moved:?}");
        assert!(
            close(cam.target - target, moved, 1e-4),
            "target travels along"
        );
        let eye = cam.eye();
        cam.fly(Vec3::new(1.0, 0.0, 0.0), 1.0, false);
        assert!(close(cam.eye() - eye, right * cam.fly_speed, 1e-3));
        let eye = cam.eye();
        cam.fly(Vec3::new(0.0, -1.0, 0.0), 1.0, true);
        assert!(close(
            cam.eye() - eye,
            Vec3::new(0.0, -cam.fly_speed * FLY_BOOST, 0.0),
            1e-3
        ));
        // The wheel sets the speed, within limits.
        let speed = cam.fly_speed;
        cam.fly_speed_scroll(100.0);
        assert!(cam.fly_speed > speed);
        cam.fly_speed_scroll(-1.0e6);
        assert_eq!(cam.fly_speed, 0.25);

        // Flying takes over from an ease in progress instead of fighting it.
        let mut cam = OrbitCam::default();
        cam.focus(Vec3::new(5.0, 0.0, 5.0), 1.0);
        cam.fly(Vec3::new(0.0, 0.0, 1.0), 0.1, false);
        let (eye, target) = (cam.eye(), cam.target);
        while cam.step(0.1) {}
        assert!(close(cam.eye(), eye, 1e-4) && close(cam.target, target, 1e-4));
    }

    #[test]
    fn dolly_and_focus_pivot() {
        let mut cam = OrbitCam::default();
        let dist = cam.dist;
        cam.dolly(-50.0);
        assert!(cam.dist < dist, "drag up dollies in");
        assert!(!cam.step(1.0 / 60.0), "immediately");
        cam.dolly(1.0e5);
        assert_eq!(cam.dist, 600.0);
        // Framing an object makes it the orbit centre: orbiting afterwards
        // turns around it, at the same distance.
        let center = Vec3::new(4.0, 1.0, -3.0);
        cam.focus(center, 1.0);
        while cam.step(0.1) {}
        cam.orbit(Vec2::new(120.0, 30.0));
        assert!(close(cam.target, center, 1e-3));
        let d = cam.eye() - center;
        assert!((d.dot(d).sqrt() - cam.dist).abs() < 1e-3);
    }

    #[test]
    fn projection_helpers_reject_points_behind_and_hit_the_ground() {
        let proj = front_proj();
        let c = proj.project(Vec3::ZERO).unwrap();
        assert!((c.x - 160.0).abs() < 1e-3 && (c.y - 120.0).abs() < 1e-3);
        assert!(proj.project(Vec3::new(0.0, 0.0, -20.0)).is_none(), "behind");
        assert!((proj.depth(Vec3::ZERO) - 10.0).abs() < 1e-3);
        assert!(close(proj.forward(), Vec3::new(0.0, 0.0, 1.0), 1e-4));
        // Y is up in the world and down on screen.
        assert!(proj.project(Vec3::new(0.0, 1.0, 0.0)).unwrap().y < c.y);
        // World size of 100 px doubles at twice the depth.
        let near = proj.world_per_px(Vec3::ZERO, 100.0);
        let far = proj.world_per_px(Vec3::new(0.0, 0.0, 10.0), 100.0);
        assert!((far / near - 2.0).abs() < 1e-3);
        // A ray below the horizon meets y = -1 in front; the centre ray is parallel.
        let g = proj.ground_point(Pos2::new(160.0, 200.0), -1.0).unwrap();
        assert!((g.y + 1.0).abs() < 1e-4 && g.z > -10.0);
        assert!(proj.ground_point(Pos2::new(160.0, 120.0), -1.0).is_none());
        assert!(
            proj.ground_point(Pos2::new(160.0, 20.0), -1.0).is_none(),
            "behind"
        );
    }

    #[test]
    fn affine_inverse_and_box_wireframe() {
        let m = Mat34::from_rotation_scale_translation(
            Vec3::new(0.3, -0.7, 1.1),
            Vec3::new(2.0, 0.5, 1.5),
            Vec3::new(4.0, -2.0, 7.0),
        );
        let p = Vec3::new(0.25, -1.5, 3.0);
        let back = inverse(&m).transform_point(m.transform_point(p));
        assert!(close(back, p, 1e-4), "{back:?}");
        assert!(close(translation(&m), Vec3::new(4.0, -2.0, 7.0), 1e-6));
        // A singular matrix inverts to zero instead of NaN.
        let z = inverse(&Mat34 { m: [[0.0; 4]; 3] });
        assert!(z.m.iter().flatten().all(|v| v.is_finite()));

        let proj = front_proj();
        assert_eq!(
            box_lines(&proj, &Mat34::IDENTITY, [-1.0; 3], [1.0; 3]).len(),
            12
        );
        // A box straddling the eye loses the edges it cannot project.
        let around_eye = Mat34::from_rotation_scale_translation(
            Vec3::ZERO,
            Vec3::ONE,
            Vec3::new(0.0, 0.0, -10.0),
        );
        assert!(box_lines(&proj, &around_eye, [-1.0; 3], [1.0; 3]).len() < 12);
        // A flat plane stays clickable (thickened).
        let t = ray_obb(
            Vec3::new(0.0, 5.0, 0.0),
            Vec3::new(0.0, -1.0, 0.0),
            &Mat34::IDENTITY,
            [-1.0, 0.0, -1.0],
            [1.0, 0.0, 1.0],
        );
        assert!((t.unwrap() - 4.99).abs() < 1e-3, "{t:?}");
    }

    #[test]
    fn gizmo_handles_are_hit_where_they_are_drawn() {
        let proj = front_proj();
        let g = Gizmo::new(&proj, Vec3::ZERO);
        let o = proj.project(g.origin).unwrap();
        assert!((g.len - proj.world_per_px(Vec3::ZERO, GIZMO_PX)).abs() < 1e-6);
        let x_tip = proj.project(Vec3::new(g.len, 0.0, 0.0)).unwrap();
        let mid_x = o + (x_tip - o) * 0.6;
        assert_eq!(
            g.hit(&proj, GizmoMode::Translate, mid_x),
            Some(Handle::Axis(0))
        );
        assert_eq!(g.hit(&proj, GizmoMode::Scale, mid_x), Some(Handle::Axis(0)));
        let y_tip = proj.project(Vec3::new(0.0, g.len, 0.0)).unwrap();
        assert_eq!(
            g.hit(&proj, GizmoMode::Translate, o + (y_tip - o) * 0.6),
            Some(Handle::Axis(1))
        );
        // The scale centre is the uniform handle; far away hits nothing.
        assert_eq!(g.hit(&proj, GizmoMode::Scale, o), Some(Handle::Uniform));
        assert_eq!(
            g.hit(&proj, GizmoMode::Translate, o + Vec2::splat(150.0)),
            None
        );
        // The Z ring (in the XY plane) faces this camera: a point on it hits.
        // (On the diagonal: the X and Y rings are edge-on lines through the axes.)
        let r = g.len * 0.85 * std::f32::consts::FRAC_1_SQRT_2;
        let on_ring = proj.project(Vec3::new(r, r, 0.0)).unwrap();
        assert_eq!(
            g.hit(&proj, GizmoMode::Rotate, on_ring),
            Some(Handle::Ring(2))
        );
        // Inside the rings, away from the edge-on ones (the axis lines), hits nothing.
        assert_eq!(
            g.hit(&proj, GizmoMode::Rotate, o + Vec2::new(20.0, -20.0)),
            None
        );
        // A gizmo behind the camera has no handles.
        let behind = Gizmo {
            origin: Vec3::new(0.0, 0.0, -20.0),
            len: 1.0,
        };
        assert_eq!(behind.hit(&proj, GizmoMode::Translate, o), None);
    }

    #[test]
    fn drags_move_scale_and_rotate_the_local_transform() {
        let proj = front_proj();
        let g = Gizmo::new(&proj, Vec3::ZERO);
        let o = proj.project(g.origin).unwrap();
        let local = Transform::default();
        let world = local.matrix();
        let px_per_unit = (proj.project(Vec3::new(g.len, 0.0, 0.0)).unwrap() - o).length() / g.len;

        // Move along X by 1.1 units: snapped to the 0.25 grid -> 1.0.
        let drag = Drag::begin(
            GizmoMode::Translate,
            Handle::Axis(0),
            &g,
            &proj,
            o,
            &world,
            local,
        )
        .unwrap();
        let to = o + Vec2::new(px_per_unit * 1.1, 0.0);
        let free = drag.update(&proj, to, false);
        assert!((free.pos[0] - 1.1).abs() < 1e-3, "{:?}", free.pos);
        assert_eq!(drag.update(&proj, to, true).pos, [1.0, 0.0, 0.0]);

        // The same handle in scale mode scales X only.
        let drag = Drag::begin(
            GizmoMode::Scale,
            Handle::Axis(0),
            &g,
            &proj,
            o,
            &world,
            local,
        )
        .unwrap();
        let t = drag.update(&proj, o + Vec2::new(px_per_unit * g.len, 0.0), true);
        assert_eq!(t.scale, [2.0, 1.0, 1.0]);
        assert_eq!(t.pos, local.pos);

        // Uniform scale grows all three axes together.
        let drag = Drag::begin(
            GizmoMode::Scale,
            Handle::Uniform,
            &g,
            &proj,
            o,
            &world,
            local,
        )
        .unwrap();
        let t = drag.update(&proj, o + Vec2::new(50.0, 0.0), false);
        assert!(t.scale[0] > 1.2 && t.scale[0] == t.scale[1] && t.scale[1] == t.scale[2]);

        // A quarter turn on the Z ring, snapped to 15 degree steps.
        let start = o + Vec2::new(40.0, 0.0);
        let drag = Drag::begin(
            GizmoMode::Rotate,
            Handle::Ring(2),
            &g,
            &proj,
            start,
            &world,
            local,
        )
        .unwrap();
        let t = drag.update(&proj, o + Vec2::new(0.0, 40.0), true);
        assert_eq!(t.rot[0], 0.0);
        assert_eq!(t.rot[1], 0.0);
        assert_eq!(t.rot[2].abs(), 90.0, "{:?}", t.rot);

        // Ground-plane move from a camera looking down.
        let cam = Camera3 {
            eye: Vec3::new(0.0, 10.0, -10.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        let down = Proj::new(
            &cam,
            (320, 240),
            Rect::from_min_size(Pos2::ZERO, Vec2::new(320.0, 240.0)),
        );
        let g = Gizmo::new(&down, Vec3::ZERO);
        let o = down.project(Vec3::ZERO).unwrap();
        let drag = Drag::begin(
            GizmoMode::Translate,
            Handle::Plane(1),
            &g,
            &down,
            o,
            &world,
            local,
        )
        .unwrap();
        let target = down.project(Vec3::new(2.0, 0.0, 1.0)).unwrap();
        let t = drag.update(&down, target, true);
        assert_eq!(t.pos, [2.0, 0.0, 1.0]);
    }

    #[test]
    fn planar_handles_move_on_their_plane_and_the_view_ring_turns_the_view_axis() {
        // Looking from (+X, +Y, -Z): all three planar squares are visible.
        let cam = Camera3 {
            eye: Vec3::new(6.0, 7.0, -8.0),
            target: Vec3::ZERO,
            fov_y: 1.0,
        };
        let rect = Rect::from_min_size(Pos2::ZERO, Vec2::new(640.0, 480.0));
        let proj = Proj::new(&cam, (640, 480), rect);
        let g = Gizmo::new(&proj, Vec3::ZERO);
        let local = Transform::default();
        let world = local.matrix();
        for (k, normal) in AXES.iter().enumerate() {
            let (u, v) = plane_axes(k);
            let mid = u * (g.len * 0.32) + v * (g.len * 0.32);
            let at = proj.project(mid).unwrap();
            assert_eq!(
                g.hit(&proj, GizmoMode::Translate, at),
                Some(Handle::Plane(k)),
                "plane {k}"
            );
            // Drag it to a point on its plane: the move lands there, the
            // normal component stays 0, and snapping rounds to the grid.
            let drag = Drag::begin(
                GizmoMode::Translate,
                Handle::Plane(k),
                &g,
                &proj,
                at,
                &world,
                local,
            )
            .unwrap();
            let goal = mid + u * 1.1 + v * 0.6;
            let to = proj.project(goal).unwrap();
            let free = drag.update(&proj, to, false);
            let p = Vec3::new(free.pos[0], free.pos[1], free.pos[2]);
            assert!(close(p, u * 1.1 + v * 0.6, 0.02), "{k}: {p:?}");
            assert!(p.dot(*normal).abs() < 1e-6, "stays on the plane");
            let snapped = drag.update(&proj, to, true);
            let p = Vec3::new(snapped.pos[0], snapped.pos[1], snapped.pos[2]);
            assert!(close(p, u * 1.0 + v * 0.5, 1e-4), "{k} snapped: {p:?}");
        }
        // Seen edge-on (the front view), the XZ and YZ squares take no
        // clicks: the X axis keeps its handle.
        let front = front_proj();
        let g = Gizmo::new(&front, Vec3::ZERO);
        let o = front.project(Vec3::ZERO).unwrap();
        let x_tip = front.project(Vec3::new(g.len, 0.0, 0.0)).unwrap();
        assert_eq!(
            g.hit(&front, GizmoMode::Translate, o + (x_tip - o) * 0.3),
            Some(Handle::Axis(0))
        );
        // The XY square faces this camera.
        let xy = front
            .project(Vec3::new(g.len * 0.32, g.len * 0.32, 0.0))
            .unwrap();
        assert_eq!(
            g.hit(&front, GizmoMode::Translate, xy),
            Some(Handle::Plane(2))
        );

        // The view ring: outside the axis rings, a screen-space circle.
        let ring_at = o + Vec2::new(0.0, -VIEW_RING_PX);
        assert_eq!(
            g.hit(&front, GizmoMode::Rotate, ring_at),
            Some(Handle::ViewRing)
        );
        // Looking down +Z it turns like the Z ring.
        let start = o + Vec2::new(VIEW_RING_PX, 0.0);
        let to = o + Vec2::new(0.0, VIEW_RING_PX);
        let turn = |h: Handle| {
            Drag::begin(GizmoMode::Rotate, h, &g, &front, start, &world, local)
                .unwrap()
                .update(&front, to, true)
                .rot
        };
        assert_eq!(turn(Handle::ViewRing), turn(Handle::Ring(2)));
        assert_eq!(turn(Handle::ViewRing)[2].abs(), 90.0);
        // Around any axis, the rotation is the matrix it says it is.
        let axis = Vec3::new(1.0, 2.0, -0.5).normalized();
        let r = rotate_euler_axis([0.0; 3], &Mat34::IDENTITY, axis, 40.0);
        let m = euler_m3(r);
        let a = [axis.x, axis.y, axis.z];
        let on_axis: Vec<f32> = (0..3)
            .map(|i| (0..3).map(|j| m[i][j] * a[j]).sum())
            .collect();
        for i in 0..3 {
            assert!((on_axis[i] - a[i]).abs() < 1e-3, "the axis is fixed");
        }
        let trace = m[0][0] + m[1][1] + m[2][2];
        assert!((((trace - 1.0) * 0.5).acos().to_degrees() - 40.0).abs() < 0.05);
    }

    #[test]
    fn rotation_under_a_rotated_parent_stays_in_world_axes() {
        // Parent turned 90 degrees around Y: a world-Y spin is still a local Y spin.
        let parent = Transform {
            rot: [0.0, 90.0, 0.0],
            ..Transform::default()
        }
        .matrix();
        let r = rotate_euler([0.0, 0.0, 0.0], &parent, 1, 30.0);
        assert!(
            (r[1] - 30.0).abs() < 0.05 && r[0].abs() < 0.05 && r[2].abs() < 0.05,
            "{r:?}"
        );
        // Helpers: segment distance and convex point-in-quad.
        let seg = (Pos2::ZERO, Pos2::new(10.0, 0.0));
        assert!((dist_to_segment(Pos2::new(5.0, 3.0), seg.0, seg.1) - 3.0).abs() < 1e-5);
        assert!((dist_to_segment(Pos2::new(-4.0, 3.0), seg.0, seg.1) - 5.0).abs() < 1e-5);
        let q = [
            Pos2::new(0.0, 0.0),
            Pos2::new(10.0, 0.0),
            Pos2::new(10.0, 10.0),
            Pos2::new(0.0, 10.0),
        ];
        assert!(point_in_quad(Pos2::new(5.0, 5.0), &q));
        assert!(!point_in_quad(Pos2::new(15.0, 5.0), &q));
        // Regression: an edge-on (zero-area) quad contains nothing. It used to
        // contain every point on its line, so in a level side view the
        // ground-plane handle stole clicks from the X axis.
        let flat = [
            Pos2::new(0.0, 5.0),
            Pos2::new(4.0, 5.0),
            Pos2::new(6.0, 5.0),
            Pos2::new(2.0, 5.0),
        ];
        assert!(!point_in_quad(Pos2::new(20.0, 5.0), &flat));
        assert!(!point_in_quad(Pos2::new(3.0, 5.0), &flat));
    }

    #[test]
    fn ray_hits_box_in_front() {
        let m =
            Mat34::from_rotation_scale_translation(Vec3::ZERO, Vec3::ONE, Vec3::new(0.0, 0.0, 5.0));
        let t = ray_obb(
            Vec3::ZERO,
            Vec3::new(0.0, 0.0, 1.0),
            &m,
            [-0.5; 3],
            [0.5; 3],
        )
        .unwrap();
        assert!((t - 4.5).abs() < 1e-3);
        assert!(
            ray_obb(
                Vec3::ZERO,
                Vec3::new(0.0, 1.0, 0.0),
                &m,
                [-0.5; 3],
                [0.5; 3]
            )
            .is_none()
        );
    }
}
