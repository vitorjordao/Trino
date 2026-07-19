//! CPU side of the sprite renderer: turning draw calls into GPU instances
//! and grouping consecutive same-texture draws into batch runs.
//!
//! Pure code — everything here is unit-testable without a GPU.

use core::ops::Range;
use trino_core::{SpriteParams, Vec2};

/// One recorded `draw_sprite` call, resolved against the sprite's pixel size.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DrawCommand {
    pub sprite: u32,
    pub pos: Vec2,
    pub size: Vec2,
    pub rotation: f32,
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub tint: [f32; 4],
}

/// GPU instance layout. Must match `shaders.wgsl` (`VsIn`) and
/// [`instance_layout`].
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct Instance {
    pub pos: [f32; 2],
    pub size: [f32; 2],
    pub rotation: f32,
    pub _pad: f32,
    pub uv0: [f32; 2],
    pub uv1: [f32; 2],
    pub tint: [f32; 4],
}

pub const INSTANCE_STRIDE: u64 = core::mem::size_of::<Instance>() as u64;

/// One recorded draw call: a sprite quad or a run of 3D triangle vertices
/// (already transformed/lit by `trino_core::render3d`; the vertices live in
/// the renderer's triangle vertex list).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Sprite(DrawCommand),
    Tris { first: u32, count: u32 },
}

/// GPU triangle vertex. Must match `shaders.wgsl` (`TriIn`).
#[repr(C)]
#[derive(Clone, Copy, Debug, bytemuck::Pod, bytemuck::Zeroable)]
pub struct TriVertex {
    pub pos: [f32; 2],
    pub color: [f32; 4],
    /// Normalized depth (0 = near) written to the depth buffer — real
    /// per-pixel occlusion; the painter batch sort is only a tie-break.
    pub depth: f32,
    pub _pad: f32,
}

pub const TRI_VERTEX_STRIDE: u64 = core::mem::size_of::<TriVertex>() as u64;

/// A contiguous stretch of same-pipeline work, in draw order.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    /// Sprite instances sharing one texture (one instanced draw call).
    Sprites { sprite: u32, instances: Range<u32> },
    /// Vertex range into the triangle vertex buffer.
    Tris { first: u32, count: u32 },
}

/// Resolve a `draw_sprite` call into a command: apply scale to the sprite's
/// native pixel size, encode flips as swapped UVs, normalize tint.
pub fn make_command(
    sprite: u32,
    pos: Vec2,
    sprite_size: (u32, u32),
    params: &SpriteParams,
) -> DrawCommand {
    let size = Vec2::new(
        sprite_size.0 as f32 * params.scale.x,
        sprite_size.1 as f32 * params.scale.y,
    );
    let (mut u0, mut u1) = (0.0, 1.0);
    let (mut v0, mut v1) = (0.0, 1.0);
    if params.flip_x {
        core::mem::swap(&mut u0, &mut u1);
    }
    if params.flip_y {
        core::mem::swap(&mut v0, &mut v1);
    }
    DrawCommand {
        sprite,
        pos,
        size,
        rotation: params.rotation,
        uv0: [u0, v0],
        uv1: [u1, v1],
        tint: [
            params.tint.r as f32 / 255.0,
            params.tint.g as f32 / 255.0,
            params.tint.b as f32 / 255.0,
            params.tint.a as f32 / 255.0,
        ],
    }
}

/// Pack commands into sprite instances plus draw segments. Consecutive
/// same-texture sprite draws share an instanced call; consecutive triangle
/// runs merge; draw order is preserved across pipeline switches.
pub fn build_segments(commands: &[Cmd]) -> (Vec<Instance>, Vec<Segment>) {
    let mut instances = Vec::new();
    let mut segments: Vec<Segment> = Vec::new();

    for cmd in commands {
        match cmd {
            Cmd::Sprite(cmd) => {
                let index = instances.len() as u32;
                instances.push(Instance {
                    pos: [cmd.pos.x, cmd.pos.y],
                    size: [cmd.size.x, cmd.size.y],
                    rotation: cmd.rotation,
                    _pad: 0.0,
                    uv0: cmd.uv0,
                    uv1: cmd.uv1,
                    tint: cmd.tint,
                });
                match segments.last_mut() {
                    Some(Segment::Sprites { sprite, instances }) if *sprite == cmd.sprite => {
                        instances.end = index + 1;
                    }
                    _ => segments.push(Segment::Sprites {
                        sprite: cmd.sprite,
                        instances: index..index + 1,
                    }),
                }
            }
            Cmd::Tris { first, count } => match segments.last_mut() {
                Some(Segment::Tris {
                    first: seg_first,
                    count: seg_count,
                }) if *seg_first + *seg_count == *first => {
                    *seg_count += count;
                }
                _ => segments.push(Segment::Tris {
                    first: *first,
                    count: *count,
                }),
            },
        }
    }
    (instances, segments)
}

#[cfg(test)]
mod tests {
    use super::*;
    use trino_core::Color;

    fn cmd(sprite: u32) -> Cmd {
        Cmd::Sprite(make_command(
            sprite,
            Vec2::ZERO,
            (16, 16),
            &SpriteParams::default(),
        ))
    }

    #[test]
    fn consecutive_same_sprite_draws_share_a_run() {
        let (instances, segments) = build_segments(&[cmd(1), cmd(1), cmd(2), cmd(1)]);
        assert_eq!(instances.len(), 4);
        assert_eq!(
            segments,
            vec![
                Segment::Sprites {
                    sprite: 1,
                    instances: 0..2
                },
                Segment::Sprites {
                    sprite: 2,
                    instances: 2..3
                },
                Segment::Sprites {
                    sprite: 1,
                    instances: 3..4
                },
            ]
        );
    }

    #[test]
    fn tris_interleave_and_merge_with_order_preserved() {
        let (instances, segments) = build_segments(&[
            cmd(1),
            Cmd::Tris { first: 0, count: 3 },
            Cmd::Tris { first: 3, count: 6 },
            cmd(1),
            Cmd::Tris { first: 9, count: 3 },
        ]);
        assert_eq!(instances.len(), 2);
        assert_eq!(
            segments,
            vec![
                Segment::Sprites {
                    sprite: 1,
                    instances: 0..1
                },
                Segment::Tris { first: 0, count: 9 },
                Segment::Sprites {
                    sprite: 1,
                    instances: 1..2
                },
                Segment::Tris { first: 9, count: 3 },
            ]
        );
    }

    #[test]
    fn scale_multiplies_native_size() {
        let params = SpriteParams {
            scale: Vec2::new(2.0, 0.5),
            ..Default::default()
        };
        let c = make_command(1, Vec2::ZERO, (16, 32), &params);
        assert_eq!(c.size, Vec2::new(32.0, 16.0));
    }

    #[test]
    fn flips_swap_uvs() {
        let params = SpriteParams {
            flip_x: true,
            flip_y: true,
            ..Default::default()
        };
        let c = make_command(1, Vec2::ZERO, (16, 16), &params);
        assert_eq!(c.uv0, [1.0, 1.0]);
        assert_eq!(c.uv1, [0.0, 0.0]);
    }

    #[test]
    fn tint_is_normalized() {
        let params = SpriteParams {
            tint: Color::rgba(255, 0, 128, 51),
            ..Default::default()
        };
        let c = make_command(1, Vec2::ZERO, (1, 1), &params);
        assert_eq!(c.tint[0], 1.0);
        assert_eq!(c.tint[1], 0.0);
        assert!((c.tint[2] - 128.0 / 255.0).abs() < 1e-6);
        assert!((c.tint[3] - 0.2).abs() < 0.01);
    }

    #[test]
    fn instance_layout_matches_wgsl_expectations() {
        // The WGSL shader reads: pos@0, size@8, rotation@16, uv0@24, uv1@32,
        // tint@40, stride 56. Guard against accidental reordering.
        assert_eq!(INSTANCE_STRIDE, 56);
        assert_eq!(core::mem::offset_of!(Instance, uv0), 24);
        assert_eq!(core::mem::offset_of!(Instance, tint), 40);
    }
}
