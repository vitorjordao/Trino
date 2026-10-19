//! High-contrast outlines on key 3D objects (issue #60): a ring of solid
//! color around an object's silhouette, the same on every target with no
//! backend code.
//!
//! The ring is a second draw of the object's own model: unlit, solid in the
//! outline's color, scaled about the object's bounding sphere so it projects
//! a few pixels larger, and pushed back along the view ray just enough that
//! every point of the copy lies behind the point it copies. The z-buffer then
//! shows the copy only around the silhouette, and where one part of the
//! object stands in front of another (a toon-style inner line). One extra
//! draw per part (an unlit draw stays on the N64's RSP), and nothing while
//! high contrast is off.
//!
//! ```ignore
//! // render: the player's parts share one sphere, so every part's copy
//! // is scaled and pushed alike and the ring hugs the whole body.
//! let ring = Outline::new(Color::rgb(255, 230, 0), self.pos, 0.8);
//! for part in parts {
//!     access::draw_outlined(renderer, &camera, part.model, &part.transform,
//!         Material::VertexLit, &part.params, settings.high_contrast.then_some(&ring));
//! }
//! // a part drawn with `draw_model_matrix`: `ring.draw(renderer, &camera, model, &m, material)`
//! ```
//!
//! Limits: geometry right behind the object (a wall it touches, the floor
//! under its feet) hides the ring there; the ring is `pixels` wide where the
//! silhouette is at the sphere's center depth, thinner where it is nearer
//! the camera and wider where it is farther; in fog the copy fades a little
//! more than the object (it is farther away).

use crate::math::{Color, Vec3};
use crate::math3d::{self, Mat34};
use crate::render::{Material, ModelId, ModelParams, Renderer, Shading, Transform3};
use crate::render3d::{Camera3, NEAR_PLANE};
use crate::shadow::ShadowCast;

/// The engine's high-contrast outline color (the high-contrast theme's
/// focus yellow).
pub const OUTLINE_COLOR: Color = Color::rgb(255, 230, 0);

/// An outline around one object (all its parts).
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Outline {
    pub color: Color,
    /// The object's bounding sphere, world space: every part of a
    /// multi-part character uses the same one.
    pub center: Vec3,
    pub radius: f32,
    /// How many pixels the ring adds around the silhouette (at the
    /// sphere's center depth), in a 240-line picture's pixels (the
    /// consoles'): a taller picture gets a ring as wide for the eye.
    pub pixels: f32,
}

impl Outline {
    /// A 2-pixel ring of `color` around the sphere at `center`.
    pub const fn new(color: Color, center: Vec3, radius: f32) -> Outline {
        Outline {
            color,
            center,
            radius,
            pixels: 2.0,
        }
    }

    /// The same, `pixels` wide.
    pub const fn with_pixels(self, pixels: f32) -> Outline {
        Outline { pixels, ..self }
    }

    /// The matrix that draws a part (at `matrix`) as this outline's copy,
    /// seen by `camera` on a picture `screen_h` pixels tall; `None` when the
    /// sphere reaches the near plane or fills most of the view (too close
    /// for a ring).
    pub fn matrix(&self, camera: &Camera3, screen_h: f32, matrix: &Mat34) -> Option<Mat34> {
        let to = self.center - camera.eye;
        let dist2 = to.dot(to);
        if dist2 <= 0.0 || self.radius <= 0.0 {
            return None;
        }
        let dist = math3d::sqrt(dist2);
        if dist - self.radius <= NEAR_PLANE * 2.0 {
            return None;
        }
        let dir = to * (1.0 / dist);
        let half = camera.fov_y * 0.5;
        let focal = math3d::cos(half) / math3d::sin(half) * screen_h * 0.5;
        // Projected radius r = R·f/d; the copy, scaled by s about the
        // center and pushed back by `push`, projects s·R·f/(d + push), and
        // that is r + pixels when s = k·(d + push)/d.
        let projected = self.radius * focal / dist;
        let pixels = self.pixels.max(0.0) * (screen_h / 240.0).max(1.0);
        let k = 1.0 + pixels / projected.max(1e-3);
        // Behind what it copies: a point `a` along the view ray from the
        // center lands at s·a + push, which stays a margin m behind `a` for
        // every |a| <= R when push - (s - 1)·R >= m; that solves to
        // push = R·(k - 1 + m/R) / (1 - R·k/d). m = 0.3·R also keeps the
        // copy of a face seen at a slant (a top face from a raised camera)
        // behind the face itself. No more than that, so a floor or wall just
        // behind the object hides as little of the ring as can be.
        let room = 1.0 - self.radius * k / dist;
        if room < 0.1 {
            return None;
        }
        let push = self.radius * (k - 0.7) / room;
        let s = k * (dist + push) / dist;
        // p -> c + s (p - c) + dir · push
        let c = self.center;
        let around = Mat34 {
            m: [
                [s, 0.0, 0.0, c.x * (1.0 - s) + dir.x * push],
                [0.0, s, 0.0, c.y * (1.0 - s) + dir.y * push],
                [0.0, 0.0, s, c.z * (1.0 - s) + dir.z * push],
            ],
        };
        Some(around.mul(matrix))
    }

    /// The copy's surface: unlit, solid in the outline's color (a black
    /// tint zeroes the vertex colors, the emissive adds the color), both
    /// faces, no shadow, no palette or texture.
    pub fn params(&self) -> ModelParams {
        ModelParams {
            shading: Shading::Unlit,
            tint: Color::BLACK,
            emissive: Color::rgb(self.color.r, self.color.g, self.color.b),
            double_sided: true,
            shadow: ShadowCast::None,
            ..ModelParams::DEFAULT
        }
    }
}

impl Outline {
    /// Draw only the ring of the part `model` at `matrix` (the picture's
    /// height is the renderer's current screen's).
    pub fn draw(
        &self,
        renderer: &mut dyn Renderer,
        camera: &Camera3,
        model: ModelId,
        matrix: &Mat34,
        material: Material,
    ) {
        let h = renderer
            .screen_size(renderer.screen())
            .map_or(renderer.caps().screen_height as f32, |s| s.y);
        if let Some(m) = self.matrix(camera, h, matrix) {
            renderer.draw_model_matrix(model, &m, material, &self.params());
        }
    }
}

/// Draw `model` at `transform` (the same `draw_model` call as without an
/// outline) and, with an outline, its ring first.
#[allow(clippy::too_many_arguments)]
pub fn draw_outlined(
    renderer: &mut dyn Renderer,
    camera: &Camera3,
    model: ModelId,
    transform: &Transform3,
    material: Material,
    params: &ModelParams,
    outline: Option<&Outline>,
) {
    if let Some(o) = outline {
        o.draw(renderer, camera, model, &transform.matrix(), material);
    }
    renderer.draw_model(model, transform, material, params);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn camera() -> Camera3 {
        Camera3 {
            eye: Vec3::new(1.0, 2.0, -6.0),
            target: Vec3::new(0.0, 0.5, 0.0),
            fov_y: 60.0 * core::f32::consts::PI / 180.0,
        }
    }

    /// Screen-space distance from the projected center, in pixels.
    fn offset(cam: &Camera3, h: f32, center: Vec3, p: Vec3) -> f32 {
        let view = cam.view();
        let focal = 1.0 / math3d::sin(cam.fov_y * 0.5) * math3d::cos(cam.fov_y * 0.5) * h * 0.5;
        let a = view.transform_point(p);
        let c = view.transform_point(center);
        let (ax, ay) = (a.x * focal / a.z, a.y * focal / a.z);
        let (cx, cy) = (c.x * focal / c.z, c.y * focal / c.z);
        math3d::sqrt((ax - cx) * (ax - cx) + (ay - cy) * (ay - cy))
    }

    #[test]
    fn the_copy_lies_behind_what_it_copies_and_rings_it_by_the_pixels() {
        let cam = camera();
        let center = Vec3::new(0.2, 0.6, 0.3);
        let ring = Outline::new(OUTLINE_COLOR, center, 0.7).with_pixels(3.0);
        let m = ring.matrix(&cam, 240.0, &Mat34::IDENTITY).unwrap();
        // Every point of the sphere's volume lands farther from the eye than
        // it was (by the margin, a tenth of the radius, along the view ray).
        for i in 0..27 {
            let u = Vec3::new(
                (i % 3) as f32 - 1.0,
                ((i / 3) % 3) as f32 - 1.0,
                (i / 9) as f32 - 1.0,
            );
            let u = if i == 13 {
                Vec3::ZERO
            } else {
                u.normalized() * 0.7
            };
            for p in [center + u, center + u * 0.5] {
                let before = p - cam.eye;
                let after = m.transform_point(p) - cam.eye;
                let gap = math3d::sqrt(after.dot(after)) - math3d::sqrt(before.dot(before));
                assert!(gap > 0.06, "point {i}: {gap}");
            }
        }
        // A point on the sphere's rim (perpendicular to the view ray)
        // projects `pixels` farther out, the center stays put.
        let dir = (center - cam.eye).normalized();
        let side = dir.cross(Vec3::new(0.0, 1.0, 0.0)).normalized();
        let rim = center + side * 0.7;
        let before = offset(&cam, 240.0, center, rim);
        let after = offset(&cam, 240.0, center, m.transform_point(rim));
        assert!((after - before - 3.0).abs() < 0.05, "{before} -> {after}");
        assert!(offset(&cam, 240.0, center, m.transform_point(center)) < 0.01);
        // On a 720-line picture the same ring is three times as many pixels.
        let m = ring.matrix(&cam, 720.0, &Mat34::IDENTITY).unwrap();
        let before = offset(&cam, 720.0, center, rim);
        let after = offset(&cam, 720.0, center, m.transform_point(rim));
        assert!((after - before - 9.0).abs() < 0.1, "{before} -> {after}");
    }

    #[test]
    fn no_ring_when_the_sphere_is_too_close() {
        let cam = camera();
        let ring = Outline::new(OUTLINE_COLOR, cam.eye + Vec3::new(0.0, 0.0, 0.5), 0.45);
        assert!(ring.matrix(&cam, 240.0, &Mat34::IDENTITY).is_none());
        let ring = Outline {
            radius: 0.0,
            ..ring
        };
        assert!(ring.matrix(&cam, 240.0, &Mat34::IDENTITY).is_none());
    }

    #[test]
    fn the_copy_is_an_unlit_shadowless_draw_in_the_color() {
        let p = Outline::new(Color::rgb(1, 2, 3), Vec3::ZERO, 1.0).params();
        assert_eq!(p.shading, Shading::Unlit);
        assert_eq!(p.tint, Color::BLACK);
        assert_eq!(p.emissive, Color::rgb(1, 2, 3));
        assert!(p.double_sided);
        assert_eq!(p.shadow, ShadowCast::None);
    }
}
