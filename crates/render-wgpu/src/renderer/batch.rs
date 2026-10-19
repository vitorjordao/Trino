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

/// Texture state of a triangle run: the sprite, tiled (models) or clamped
/// (billboards), its filter and detail layer (`trino_core`'s backend
/// helper).
pub use trino_core::render3d::TexBind as TexKey;

/// Which triangle pipeline a run of 3D triangles takes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TriKind {
    /// Vertex colors × the texture.
    Plain,
    /// Shaded per pixel (local lights, specular, wetness).
    Lit,
    /// A water surface (ADR 0015): translucent, after the batch's others.
    Water,
    /// Blob and planar shadows (ADR 0025, `shadow.rs`): blended over the
    /// batch's opaque triangles, before its water.
    Shadow,
    /// Planar shadows drawn once per pixel: the alpha mark, then the blend
    /// over the same vertices.
    ShadowMark,
    ShadowApply,
}

/// One recorded draw call: a sprite quad or a run of 3D triangle vertices
/// (already transformed/lit by `trino_core::render3d`; the vertices live in
/// the renderer's triangle vertex list).
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Cmd {
    Sprite(DrawCommand),
    /// `texture`: sprite handle sampled by these triangles (None = vertex
    /// colors only). `kind`: the pipeline.
    Tris {
        first: u32,
        count: u32,
        texture: Option<TexKey>,
        kind: TriKind,
    },
    /// Instanced draw calls of the GPU T&L path (`gpu_tnl::GpuBatch`
    /// indices): a batch's opaque GPU draws, ahead of its CPU triangles.
    Meshes {
        first: u32,
        count: u32,
    },
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
    /// Clip-space w: the view depth for textured triangles (perspective-
    /// correct UVs), 1.0 for vertex-color-only ones (affine, as always).
    pub w: f32,
    pub uv: [f32; 2],
    /// Per-pixel lighting inputs (the lit pipeline only; zero elsewhere):
    /// world position, world normal (zero = leave unlit), the unlit color
    /// (`SurfaceVert::base`), gloss 0..1, and the light set index plus 8 ×
    /// the mask of the local lights that reach the draw.
    pub world: [f32; 3],
    pub normal: [f32; 3],
    pub base: [f32; 4],
    pub gloss: f32,
    pub set: f32,
    /// Texture sampling: 1 = point filter (the N64 look's 3-point
    /// otherwise), and the detail layer's UV scale (0 = no detail).
    pub tex: [f32; 2],
}

pub const TRI_VERTEX_STRIDE: u64 = core::mem::size_of::<TriVertex>() as u64;

/// A contiguous stretch of same-pipeline work, in draw order.
#[derive(Clone, Debug, PartialEq)]
pub enum Segment {
    /// Sprite instances sharing one texture (one instanced draw call).
    Sprites { sprite: u32, instances: Range<u32> },
    /// Vertex range into the triangle vertex buffer, one texture.
    Tris {
        first: u32,
        count: u32,
        texture: Option<TexKey>,
        kind: TriKind,
    },
    /// GPU T&L draw calls (`gpu_tnl::GpuBatch` indices).
    Meshes { first: u32, count: u32 },
}

/// Resolve a `draw_sprite` call into a command: apply scale to the sprite's
/// native pixel size, encode flips as swapped UVs, normalize tint.
pub fn make_command(
    sprite: u32,
    pos: Vec2,
    sprite_size: (u32, u32),
    params: &SpriteParams,
) -> DrawCommand {
    let (tw, th) = (sprite_size.0 as f32, sprite_size.1 as f32);
    // Region in sprite pixels (clamped to the texture), else the whole sprite.
    let (rx, ry, rw, rh) = match params.region {
        Some(r) => {
            let x0 = r.pos.x.clamp(0.0, tw);
            let y0 = r.pos.y.clamp(0.0, th);
            let x1 = (r.pos.x + r.size.x).clamp(x0, tw);
            let y1 = (r.pos.y + r.size.y).clamp(y0, th);
            (x0, y0, x1 - x0, y1 - y0)
        }
        None => (0.0, 0.0, tw, th),
    };
    let size = Vec2::new(rw * params.scale.x, rh * params.scale.y);
    let (mut u0, mut u1) = (rx / tw.max(1.0), (rx + rw) / tw.max(1.0));
    let (mut v0, mut v1) = (ry / th.max(1.0), (ry + rh) / th.max(1.0));
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
            Cmd::Tris {
                first,
                count,
                texture,
                kind,
            } => match segments.last_mut() {
                Some(Segment::Tris {
                    first: seg_first,
                    count: seg_count,
                    texture: seg_tex,
                    kind: seg_kind,
                }) if *seg_first + *seg_count == *first
                    && seg_tex == texture
                    && seg_kind == kind =>
                {
                    *seg_count += count;
                }
                _ => segments.push(Segment::Tris {
                    first: *first,
                    count: *count,
                    texture: *texture,
                    kind: *kind,
                }),
            },
            Cmd::Meshes { first, count } => match segments.last_mut() {
                Some(Segment::Meshes {
                    first: seg_first,
                    count: seg_count,
                }) if *seg_first + *seg_count == *first => *seg_count += count,
                _ => segments.push(Segment::Meshes {
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

    fn tex5() -> TexKey {
        TexKey::model(
            5,
            &trino_core::ModelParams::DEFAULT,
            trino_core::TexFilter::Point,
        )
    }

    #[test]
    fn tris_interleave_and_merge_with_order_preserved() {
        let tris = |first, count| Cmd::Tris {
            first,
            count,
            texture: None,
            kind: TriKind::Plain,
        };
        let (instances, segments) = build_segments(&[
            cmd(1),
            tris(0, 3),
            tris(3, 6),
            cmd(1),
            tris(9, 3),
            // Same vertex run continues but with a texture: new segment.
            Cmd::Tris {
                first: 12,
                count: 3,
                texture: Some(tex5()),
                kind: TriKind::Plain,
            },
            // Per-pixel lit: another pipeline, another segment.
            Cmd::Tris {
                first: 15,
                count: 3,
                texture: Some(tex5()),
                kind: TriKind::Lit,
            },
        ]);
        assert_eq!(instances.len(), 2);
        assert_eq!(
            segments,
            vec![
                Segment::Sprites {
                    sprite: 1,
                    instances: 0..1
                },
                Segment::Tris {
                    first: 0,
                    count: 9,
                    texture: None,
                    kind: TriKind::Plain
                },
                Segment::Sprites {
                    sprite: 1,
                    instances: 1..2
                },
                Segment::Tris {
                    first: 9,
                    count: 3,
                    texture: None,
                    kind: TriKind::Plain
                },
                Segment::Tris {
                    first: 12,
                    count: 3,
                    texture: Some(tex5()),
                    kind: TriKind::Plain
                },
                Segment::Tris {
                    first: 15,
                    count: 3,
                    texture: Some(tex5()),
                    kind: TriKind::Lit
                },
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
    fn region_selects_uvs_and_size() {
        let params = SpriteParams {
            region: Some(trino_core::Rect::new(8.0, 4.0, 8.0, 12.0)),
            scale: Vec2::new(2.0, 1.0),
            ..Default::default()
        };
        let c = make_command(1, Vec2::ZERO, (32, 16), &params);
        assert_eq!(c.size, Vec2::new(16.0, 12.0));
        assert_eq!(c.uv0, [0.25, 0.25]);
        assert_eq!(c.uv1, [0.5, 1.0]);
        // Regions past the texture edge clamp instead of sampling garbage.
        let params = SpriteParams {
            region: Some(trino_core::Rect::new(24.0, 0.0, 16.0, 16.0)),
            ..Default::default()
        };
        let c = make_command(1, Vec2::ZERO, (32, 16), &params);
        assert_eq!(c.size, Vec2::new(8.0, 16.0));
        assert_eq!(c.uv1, [1.0, 1.0]);
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
    fn tri_vertex_layout_matches_the_pipelines() {
        // shaders.wgsl TriIn/LitIn locations: pos@0, color@8, depth@24,
        // w@28, uv@32, world@40, normal@52, base@64, gloss@80, set@84,
        // tex@88.
        assert_eq!(TRI_VERTEX_STRIDE, 96);
        assert_eq!(core::mem::offset_of!(TriVertex, tex), 88);
        assert_eq!(core::mem::offset_of!(TriVertex, world), 40);
        assert_eq!(core::mem::offset_of!(TriVertex, normal), 52);
        assert_eq!(core::mem::offset_of!(TriVertex, base), 64);
        assert_eq!(core::mem::offset_of!(TriVertex, gloss), 80);
        assert_eq!(core::mem::offset_of!(TriVertex, set), 84);
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
