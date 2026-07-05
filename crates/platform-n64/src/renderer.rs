//! RDP sprite renderer via the shim's `rdpq` wrappers, plus gouraud
//! triangles for the engine's software-T&L 3D (`trino_core::render3d`).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Mesh, ScreenTri};
use trino_core::{
    Caps, Color, Material, ModelId, Renderer, SpriteId, SpriteParams, Transform3, Vec2,
};

use crate::ffi;

struct SpriteEntry {
    ptr: *mut c_void,
}

pub struct N64Renderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    meshes: BTreeMap<u32, Vec<u8>>,
    tri_scratch: Vec<ScreenTri>,
    camera: Camera3,
    caps: Caps,
}

impl N64Renderer {
    pub fn new() -> Self {
        N64Renderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            tri_scratch: Vec::new(),
            camera: Camera3::default(),
            caps: Caps::N64,
        }
    }

    /// Register a `sprite_t` loaded from the DFS under a stable handle.
    pub(crate) fn register(&mut self, id: u32, ptr: *mut c_void) {
        self.sprites.insert(id, SpriteEntry { ptr });
    }

    /// Register a TMDL mesh blob under a stable handle.
    pub(crate) fn register_mesh(&mut self, id: u32, tmdl: Vec<u8>) {
        if Mesh::from_tmdl(&tmdl).is_ok() {
            self.meshes.insert(id, tmdl);
        }
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

    fn set_camera(&mut self, camera: &Camera3) {
        self.camera = *camera;
    }

    fn draw_model(&mut self, model: ModelId, transform: &Transform3, _material: Material) {
        let Some(tmdl) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_tmdl(tmdl).expect("validated on register");
        let max_tris = mesh.index_count / 3;
        self.tri_scratch.resize(
            max_tris,
            ScreenTri {
                pts: [Vec2::ZERO; 3],
                colors: [Color::WHITE; 3],
                depth: 0.0,
            },
        );
        let n = trino_core::render3d::tessellate(
            &mesh,
            &transform.matrix(),
            &self.camera,
            &DEFAULT_LIGHT,
            Vec2::new(320.0, 240.0),
            &mut self.tri_scratch,
        );
        if n == 0 {
            return;
        }
        unsafe { ffi::trino_3d_begin() }
        for tri in &self.tri_scratch[..n] {
            let pts = [
                tri.pts[0].x,
                tri.pts[0].y,
                tri.pts[1].x,
                tri.pts[1].y,
                tri.pts[2].x,
                tri.pts[2].y,
            ];
            let mut colors = [0u8; 12];
            for (i, c) in tri.colors.iter().enumerate() {
                colors[i * 4..i * 4 + 4].copy_from_slice(&[c.r, c.g, c.b, c.a]);
            }
            unsafe { ffi::trino_tri(pts.as_ptr(), colors.as_ptr()) }
        }
    }

    fn end_frame(&mut self) {
        unsafe { ffi::trino_frame_end() }
    }
}
