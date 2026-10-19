//! Blob and planar shadows on the PC renderer ([`trino_core::shadow`], ADR
//! 0024): the triangles come from the core (blob quads, flattened casters,
//! cut to their receiver and moved a few depth steps in front of it); this
//! module owns the pipelines that blend them over the 3D.
//!
//! - `blend`: the shadow color over the surface by the triangle's alpha,
//!   depth-tested, never written — blobs everywhere, and planar shadows on
//!   the N64 profile (overlapping parts darken twice, as on the console).
//! - `mark` + `apply`: planar shadows drawn once per pixel, as the 3DS's
//!   stencil draws them: `mark` keeps `1 - strength` in the target's alpha
//!   (the strongest over overlaps), `apply` blends the color by it and
//!   resets the alpha to 1, so a second fragment there changes nothing.
//!   The scene's alpha is 1 everywhere before (cleared opaque, every blend
//!   keeps it), and after.
//!
//! Built on the first frame with shadows (most renderers never draw any),
//! once more for a multisampled target.

use trino_core::{Color, SpriteId};

use super::{OFFSCREEN_FORMAT, SceneSources, TRI_VERTEX_STRIDE, tri_attributes};

/// The blob texture's handle (the PC's sprite table; never a game asset's:
/// game handles are path hashes).
pub(crate) const BLOB_SPRITE: SpriteId = SpriteId::from_path("\0trino/shadow-blob");

/// What a pending shadow triangle is ([`super::PcRenderer`]'s shadow
/// bucket).
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub(crate) enum ShadowRun {
    /// A blob: the blob texture, blended.
    Blob,
    /// A planar shadow, blended (the N64 profile).
    Planar,
    /// A planar shadow drawn once per pixel (`mark` then `apply`).
    PlanarOnce,
}

/// The three shadow pipelines for one sample count.
pub(crate) struct ShadowPipes {
    pub blend: wgpu::RenderPipeline,
    pub mark: wgpu::RenderPipeline,
    pub apply: wgpu::RenderPipeline,
}

impl ShadowPipes {
    pub fn new(device: &wgpu::Device, src: &SceneSources, samples: u32) -> ShadowPipes {
        let attrs = tri_attributes();
        let make = |label: &str, fragment: &str, blend: wgpu::BlendState, mask| {
            device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
                label: Some(label),
                layout: Some(&src.tri_layout),
                vertex: wgpu::VertexState {
                    module: &src.shader,
                    entry_point: Some("vs_tri"),
                    compilation_options: Default::default(),
                    buffers: &[wgpu::VertexBufferLayout {
                        array_stride: TRI_VERTEX_STRIDE,
                        step_mode: wgpu::VertexStepMode::Vertex,
                        attributes: &attrs,
                    }],
                },
                fragment: Some(wgpu::FragmentState {
                    module: &src.shader,
                    entry_point: Some(fragment),
                    compilation_options: Default::default(),
                    targets: &[Some(wgpu::ColorTargetState {
                        format: OFFSCREEN_FORMAT,
                        blend: Some(blend),
                        write_mask: mask,
                    })],
                }),
                primitive: wgpu::PrimitiveState::default(),
                // On the receiver, in front of it by the core's depth
                // offset: tested, never written.
                depth_stencil: Some(wgpu::DepthStencilState {
                    format: wgpu::TextureFormat::Depth32Float,
                    depth_write_enabled: Some(false),
                    depth_compare: Some(wgpu::CompareFunction::LessEqual),
                    stencil: wgpu::StencilState::default(),
                    bias: wgpu::DepthBiasState::default(),
                }),
                multisample: wgpu::MultisampleState {
                    count: samples,
                    ..Default::default()
                },
                multiview_mask: None,
                cache: None,
            })
        };
        let keep = wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::One,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        };
        ShadowPipes {
            blend: make(
                "trino-shadow-blend",
                "fs_shadow",
                wgpu::BlendState::ALPHA_BLENDING,
                wgpu::ColorWrites::ALL,
            ),
            mark: make(
                "trino-shadow-mark",
                "fs_shadow_mark",
                wgpu::BlendState {
                    color: keep,
                    // Min takes factors of one (WebGPU's rule).
                    alpha: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::One,
                        dst_factor: wgpu::BlendFactor::One,
                        operation: wgpu::BlendOperation::Min,
                    },
                },
                wgpu::ColorWrites::ALPHA,
            ),
            apply: make(
                "trino-shadow-apply",
                "fs_shadow_apply",
                wgpu::BlendState {
                    color: wgpu::BlendComponent {
                        src_factor: wgpu::BlendFactor::OneMinusDstAlpha,
                        dst_factor: wgpu::BlendFactor::DstAlpha,
                        operation: wgpu::BlendOperation::Add,
                    },
                    alpha: keep,
                },
                wgpu::ColorWrites::ALL,
            ),
        }
    }
}

/// The blob texture's RGBA: white, the core's falloff in alpha.
pub(crate) fn blob_rgba(softness: u8) -> Vec<u8> {
    let n = trino_core::shadow::BLOB_TEXTURE_SIZE;
    let mut a =
        [0u8; trino_core::shadow::BLOB_TEXTURE_SIZE * trino_core::shadow::BLOB_TEXTURE_SIZE];
    trino_core::shadow::blob_texels(softness, &mut a);
    let mut rgba = Vec::with_capacity(n * n * 4);
    for v in a {
        rgba.extend_from_slice(&[255, 255, 255, v]);
    }
    rgba
}

/// What the shadows show (`PcRenderer::set_shadow_view`, the editor's
/// Scene view, `TRINO_SHADOWS`).
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub enum ShadowView {
    /// As the game sets them, at the profile's tier.
    #[default]
    Game,
    /// None, whatever the game sets.
    Off,
    /// The game's (or the default settings in a frame without any) in a
    /// loud color at full strength: where each caster's shadow lands.
    Debug,
}

impl ShadowView {
    /// `TRINO_SHADOWS` values: `off`, `debug` (`game`/`tier`: as set).
    pub fn parse(s: &str) -> Option<ShadowView> {
        match s.to_ascii_lowercase().as_str() {
            "game" | "tier" | "auto" | "on" => Some(ShadowView::Game),
            "off" | "0" | "false" => Some(ShadowView::Off),
            "debug" => Some(ShadowView::Debug),
            _ => None,
        }
    }

    pub const ALL: [ShadowView; 3] = [ShadowView::Game, ShadowView::Off, ShadowView::Debug];

    /// [`ShadowView::Debug`]'s color (the editor's overlay of receivers
    /// and casters matches it).
    pub const DEBUG_COLOR: Color = DEBUG_COLOR;

    pub const fn label(self) -> &'static str {
        match self {
            ShadowView::Game => "As tier",
            ShadowView::Off => "Off",
            ShadowView::Debug => "Debug",
        }
    }

    /// The settings a frame draws with: `game` is what the game set.
    pub fn resolve(
        self,
        game: Option<trino_core::ShadowSettings>,
    ) -> Option<trino_core::ShadowSettings> {
        match self {
            ShadowView::Game => game,
            ShadowView::Off => None,
            ShadowView::Debug => Some(trino_core::ShadowSettings {
                color: DEBUG_COLOR,
                softness: 0,
                ..game.unwrap_or_default()
            }),
        }
    }
}

/// [`ShadowView::Debug`]'s shadow color.
pub const DEBUG_COLOR: Color = Color::rgba(255, 0, 200, 230);

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn views_parse_and_resolve() {
        assert_eq!(ShadowView::parse("debug"), Some(ShadowView::Debug));
        assert_eq!(ShadowView::parse("OFF"), Some(ShadowView::Off));
        assert_eq!(ShadowView::parse("tier"), Some(ShadowView::Game));
        assert_eq!(ShadowView::parse("nope"), None);
        let s = trino_core::ShadowSettings::DEFAULT;
        assert_eq!(ShadowView::Game.resolve(Some(s)), Some(s));
        assert_eq!(ShadowView::Game.resolve(None), None);
        assert_eq!(ShadowView::Off.resolve(Some(s)), None);
        let d = ShadowView::Debug.resolve(None).unwrap();
        assert_eq!(d.color, DEBUG_COLOR);
        assert_eq!(d.tier_max, s.tier_max);
    }

    #[test]
    fn the_blob_texture_is_white_with_the_falloff_in_alpha() {
        let rgba = blob_rgba(160);
        let n = trino_core::shadow::BLOB_TEXTURE_SIZE;
        assert_eq!(rgba.len(), n * n * 4);
        assert!(rgba.chunks(4).all(|p| p[..3] == [255, 255, 255]));
        let mid = ((n / 2) * n + n / 2) * 4;
        assert!(rgba[mid + 3] > 250 && rgba[3] == 0);
    }
}
