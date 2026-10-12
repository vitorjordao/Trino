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
    goal_target: Vec3,
    goal_yaw: f32,
    goal_pitch: f32,
    goal_dist: f32,
}

impl Default for OrbitCam {
    fn default() -> Self {
        let mut cam = OrbitCam {
            target: Vec3::new(0.0, 0.5, 0.0),
            yaw: 0.6,
            pitch: 0.45,
            dist: 12.0,
            fov: 55f32.to_radians(),
            goal_target: Vec3::ZERO,
            goal_yaw: 0.0,
            goal_pitch: 0.0,
            goal_dist: 0.0,
        };
        cam.snap_goals();
        cam
    }
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

    pub fn eye(&self) -> Vec3 {
        self.target + Self::offset(self.yaw, self.pitch) * self.dist
    }

    pub fn camera(&self) -> Camera3 {
        Camera3 {
            eye: self.eye(),
            target: self.target,
            fov_y: self.fov,
        }
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
        self.goal_yaw += delta.x * 0.008;
        self.goal_pitch = (self.goal_pitch + delta.y * 0.008).clamp(-1.54, 1.54);
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

    /// Ease to frame a sphere.
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
        let (o, d) = self.ray(pos);
        if d.y.abs() < 1e-5 {
            return None;
        }
        let t = (h - o.y) / d.y;
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
    /// Move on the ground plane (XZ).
    Plane,
    Ring(usize),
    Uniform,
}

const AXES: [Vec3; 3] = [
    Vec3::new(1.0, 0.0, 0.0),
    Vec3::new(0.0, 1.0, 0.0),
    Vec3::new(0.0, 0.0, 1.0),
];
const AXIS_COLORS: [Color32; 3] = [c::AXIS_X, c::AXIS_Y, c::AXIS_Z];
const GIZMO_PX: f32 = 90.0;

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
        let (u, v) = match axis {
            0 => (AXES[1], AXES[2]),
            1 => (AXES[2], AXES[0]),
            _ => (AXES[0], AXES[1]),
        };
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
                    let q = self.plane_quad(proj);
                    if q.len() == 4 && point_in_quad(mouse, &q) {
                        return Some(Handle::Plane);
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
                .map(Handle::Ring),
        }
    }

    fn plane_quad(&self, proj: &Proj) -> Vec<Pos2> {
        let (a, b) = (self.len * 0.22, self.len * 0.42);
        [(a, a), (b, a), (b, b), (a, b)]
            .iter()
            .filter_map(|&(x, z)| proj.project(self.origin + Vec3::new(x, 0.0, z)))
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
                    let q = self.plane_quad(proj);
                    if q.len() == 4 {
                        let col = lit(Handle::Plane, c::AXIS_Y);
                        painter.add(egui::Shape::convex_polygon(
                            q.clone(),
                            col.gamma_multiply(0.25),
                            Stroke::new(1.0, col),
                        ));
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
    world: Mat34,
    origin_px: Pos2,
    axis_px: Vec2,
    px_per_unit: f32,
    plane_start: Option<Vec3>,
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
        Some(Drag {
            handle,
            start_mouse: mouse,
            start: local,
            parent,
            world: *world,
            origin_px,
            axis_px,
            px_per_unit: px_per_unit.max(1e-6),
            plane_start: proj.ground_point(mouse, gizmo.origin.y),
            translate: mode == GizmoMode::Translate,
            len: gizmo.len,
        })
    }

    /// New local transform for the current mouse position.
    pub fn update(&self, proj: &Proj, mouse: Pos2, snap: bool) -> Transform {
        let mut t = self.start;
        let parent_inv = inverse(&self.parent);
        let snapf = |v: f32, step: f32| if snap { (v / step).round() * step } else { v };
        match self.handle {
            Handle::Axis(k) if self.translate => {
                let s = (mouse - self.start_mouse).dot(self.axis_px) / self.px_per_unit;
                let s = snapf(s, 0.25);
                let local = parent_inv.transform_dir(AXES[k] * s);
                t.pos = [
                    self.start.pos[0] + local.x,
                    self.start.pos[1] + local.y,
                    self.start.pos[2] + local.z,
                ];
            }
            Handle::Plane => {
                if let (Some(a), Some(b)) = (
                    self.plane_start,
                    proj.ground_point(mouse, translation(&self.world).y),
                ) {
                    let d = b - a;
                    let d = Vec3::new(snapf(d.x, 0.25), 0.0, snapf(d.z, 0.25));
                    let local = parent_inv.transform_dir(d);
                    t.pos = [
                        self.start.pos[0] + local.x,
                        self.start.pos[1] + local.y,
                        self.start.pos[2] + local.z,
                    ];
                }
            }
            Handle::Axis(k) => {
                // Scale along the local axis k.
                let s = (mouse - self.start_mouse).dot(self.axis_px) / self.px_per_unit;
                let f = (1.0 + s / self.len).max(0.01);
                t.scale[k] = snapf(self.start.scale[k] * f, 0.05).max(0.001);
            }
            Handle::Uniform => {
                let d = mouse - self.start_mouse;
                let f = (1.0 + (d.x - d.y) * 0.006).max(0.01);
                for k in 0..3 {
                    t.scale[k] = snapf(self.start.scale[k] * f, 0.05).max(0.001);
                }
            }
            Handle::Ring(k) => {
                let a0 = (self.start_mouse - self.origin_px).angle();
                let a1 = (mouse - self.origin_px).angle();
                let mut delta = a1 - a0;
                // Toward-viewer axes rotate CCW on screen for positive angles.
                let toward = AXES[k].dot(proj.forward()) < 0.0;
                delta = if toward { -delta } else { delta };
                let deg = snapf(delta.to_degrees(), 15.0);
                t.rot = rotate_euler(self.start.rot, &self.parent, k, deg);
            }
        }
        t
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
    let mut e = [0.0f32; 3];
    e[axis] = delta_deg;
    let world_delta = euler_m3(e);
    // local' = P^T · Δ · P · local
    let local = m3_mul(
        &m3_t(&p),
        &m3_mul(&world_delta, &m3_mul(&p, &euler_m3(start))),
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
    true
}

/// Small XYZ orientation widget (top-right of the scene view). Returns the
/// axis clicked (positive = 0..3, negative = 3..6) to snap the view.
pub fn axis_widget(ui: &egui::Ui, proj: &Proj, center: Pos2) -> Option<usize> {
    let painter = ui.painter();
    painter.circle_filled(center, 34.0, c::BG0.gamma_multiply(0.55));
    let view = &proj.view.m;
    let mut items: Vec<(f32, usize, Vec2)> = Vec::new();
    for k in 0..3usize {
        let (rx, ry, rz) = (view[0], view[1], view[2]);
        let v = Vec2::new(rx[k], -ry[k]); // screen-space axis
        let depth = rz[k];
        items.push((depth, k, v));
        items.push((-depth, k + 3, -v));
    }
    items.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
    let mut clicked = None;
    let pointer = ui.input(|i| i.pointer.hover_pos());
    let pressed = ui.input(|i| i.pointer.primary_clicked());
    for (_, idx, v) in items {
        let k = idx % 3;
        let tip = center + v * 24.0;
        let positive = idx < 3;
        let col = AXIS_COLORS[k];
        if positive {
            painter.line_segment([center, tip], Stroke::new(2.0, col));
        }
        let hovered = pointer.is_some_and(|p| (p - tip).length() < 8.0);
        let fill = if positive {
            col
        } else {
            col.gamma_multiply(0.35)
        };
        painter.circle(
            tip,
            if hovered { 8.0 } else { 7.0 },
            fill,
            Stroke::new(1.0, if hovered { Color32::WHITE } else { col }),
        );
        if positive {
            painter.text(
                tip,
                egui::Align2::CENTER_CENTER,
                ["X", "Y", "Z"][k],
                egui::FontId::new(10.0, crate::theme::bold()),
                c::BG0,
            );
        }
        if hovered && pressed {
            clicked = Some(idx);
        }
    }
    clicked
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
