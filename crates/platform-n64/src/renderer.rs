//! RDP sprite renderer via the shim's `rdpq` wrappers.

use alloc::collections::BTreeMap;
use core::ffi::c_void;

use trino_core::{
    Caps, Color, Material, ModelId, Renderer, SpriteId, SpriteParams, Transform3, Vec2,
};

use crate::ffi;

struct SpriteEntry {
    ptr: *mut c_void,
}

pub struct N64Renderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    caps: Caps,
}

impl N64Renderer {
    pub fn new() -> Self {
        N64Renderer {
            sprites: BTreeMap::new(),
            caps: Caps::N64,
        }
    }

    /// Register a `sprite_t` loaded from the DFS under a stable handle.
    pub(crate) fn register(&mut self, id: u32, ptr: *mut c_void) {
        self.sprites.insert(id, SpriteEntry { ptr });
    }
}

impl Default for N64Renderer {
    fn default() -> Self {
        Self::new()
    }
}

fn pack_color(c: Color) -> u32 {
    ((c.r as u32) << 24) | ((c.g as u32) << 16) | ((c.b as u32) << 8) | c.a as u32
}

impl Renderer for N64Renderer {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn begin_frame(&mut self, clear: Color) {
        unsafe { ffi::trino_frame_begin(pack_color(clear)) }
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        let Some(entry) = self.sprites.get(&sprite.0) else {
            return;
        };
        let blit = ffi::TrinoBlit {
            x: pos.x,
            y: pos.y,
            scale_x: params.scale.x,
            scale_y: params.scale.y,
            theta: params.rotation,
            flip_x: params.flip_x as u32,
            flip_y: params.flip_y as u32,
            tint: pack_color(params.tint),
        };
        unsafe { ffi::trino_sprite_blit(entry.ptr, &blit) }
    }

    fn draw_model(&mut self, _model: ModelId, _transform: &Transform3, _material: Material) {
        unimplemented!("draw_model lands in Fase 7 (3D/tiny3d)");
    }

    fn end_frame(&mut self) {
        unsafe { ffi::trino_frame_end() }
    }
}
