//! Drawing into a texture (Bongo-Kart #184, ADR 0046): a rear-view mirror,
//! a stadium screen showing the leader, a live monitor on a desk.
//!
//! A [`RenderTarget`] names a texture by a logical path, like an asset: its
//! [`sprite`](RenderTarget::sprite) handle is what
//! [`Renderer::draw_sprite`](crate::Renderer::draw_sprite) and
//! [`ModelParams::texture`](crate::ModelParams::texture) take.
//! [`Renderer::begin_target`](crate::Renderer::begin_target) sends the next
//! draws (2D and 3D) into it and
//! [`end_target`](crate::Renderer::end_target) renders them: draws made
//! after `end_target` that sample it show this frame's picture, and it keeps
//! that picture until it is drawn again.
//!
//! - **Targets first:** a frame draws its targets before its first draw on
//!   the screen. A `begin_target` after a screen draw is refused (the PC's
//!   strict mode panics): the screen's pass can't be split around another
//!   one without breaking its post stack and ambient occlusion.
//! - **The camera is the renderer's:** set the target's camera before its
//!   3D and the screen's again after `end_target`, as renderer state
//!   outlives a target the way it outlives frames. 3D projects to the
//!   target's size, and 2D coordinates are the target's pixels.
//! - **What a target leaves out:** it is one single-sampled pass at its own
//!   size: no post stack, no ambient occlusion, no shadows of any tier, the
//!   per-vertex water instead of the per-pixel one, and no color-vision
//!   passes (they run over the screen that shows it).
//! - **Opaque:** the clear color's alpha is ignored; a target has one mip
//!   level.
//! - **Where:** [`Caps::max_target_texels`] above 0: the PC's native profile
//!   and the Web. The consoles answer `false` for now, and so do the PC's
//!   console profiles (ADR 0046 §5: the N64 could draw into a TMEM-sized
//!   surface, the 3DS into a citro3d target). A game falls back there: a
//!   live minimap as a [`DynamicMesh`](crate::DynamicMesh), a mirror as a
//!   schematic.

use crate::caps::Caps;
use crate::render::SpriteId;

/// A texture the renderer draws into (see the module's docs).
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct RenderTarget {
    /// Its handle: what `draw_sprite` and `ModelParams::texture` name.
    pub sprite: SpriteId,
    pub width: u16,
    pub height: u16,
}

impl RenderTarget {
    /// The target `name` (a logical path such as `"targets/mirror"`: its
    /// handle is that path's asset id) of `width × height` pixels.
    pub const fn new(name: &str, width: u16, height: u16) -> RenderTarget {
        RenderTarget {
            sprite: SpriteId::from_path(name),
            width,
            height,
        }
    }

    /// Its pixels (what [`Caps::max_target_texels`] caps).
    pub const fn texels(&self) -> u32 {
        self.width as u32 * self.height as u32
    }

    /// Can a target with `caps` draw it: both sides at least 1 and at most
    /// [`Caps::max_target_texels`] pixels (never where that is 0)?
    pub const fn fits(&self, caps: &Caps) -> bool {
        self.width > 0 && self.height > 0 && self.texels() <= caps.max_target_texels
    }

    /// The same target at another size (a mirror following the window).
    pub const fn sized(self, width: u16, height: u16) -> RenderTarget {
        RenderTarget {
            width,
            height,
            ..self
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_target_fits_where_the_caps_take_it() {
        let mirror = RenderTarget::new("targets/mirror", 480, 160);
        assert_eq!(mirror.sprite, SpriteId::from_path("targets/mirror"));
        assert_eq!(mirror.texels(), 76_800);
        assert!(mirror.fits(&Caps::PC) && mirror.fits(&Caps::WEB));
        for console in [Caps::N64, Caps::N64_EXPANDED, Caps::N3DS, Caps::N3DS_NEW] {
            assert!(!mirror.fits(&console) && console.max_targets == 0);
        }
        assert!(!mirror.sized(0, 160).fits(&Caps::PC));
        assert!(!mirror.sized(2048, 1024).fits(&Caps::PC));
        assert!(mirror.sized(1024, 1024).fits(&Caps::PC));
        assert_eq!(mirror.sized(64, 32).sprite, mirror.sprite);
    }
}
