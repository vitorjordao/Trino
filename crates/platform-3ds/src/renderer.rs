//! citro2d sprite renderer (top screen, 400x240) via the shim, plus gouraud
//! triangles for the engine's software-T&L 3D (`trino_core::render3d`).

use alloc::collections::BTreeMap;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Mesh, ScreenTri};
use trino_core::{
    Caps, Color, Material, ModelId, ModelParams, Renderer, SpriteId, SpriteParams, Transform3, Vec2,
};

use crate::ffi;

struct SpriteEntry {
    ptr: *mut c_void,
}

pub struct N3dsRenderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    meshes: BTreeMap<u32, Vec<u8>>,
    tri_scratch: Vec<ScreenTri>,
    /// Current model batch: consecutive `draw_model` calls depth-sort
    /// together; flushed on sprite draws, camera changes and `end_frame`.
    pending_tris: Vec<ScreenTri>,
    camera: Camera3,
    caps: Caps,
}

impl N3dsRenderer {
    pub fn new() -> Self {
        N3dsRenderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            tri_scratch: Vec::new(),
            pending_tris: Vec::new(),
            camera: Camera3::default(),
            caps: Caps::N3DS,
        }
    }

    /// Depth-sort and rasterize the pending model batch (painter's order
    /// across every mesh of the batch).
    fn flush_model_batch(&mut self) {
        if self.pending_tris.is_empty() {
            return;
        }
        self.pending_tris.sort_unstable_by(|a, b| {
            b.depth
                .partial_cmp(&a.depth)
                .unwrap_or(core::cmp::Ordering::Equal)
        });
        unsafe { ffi::trino_3d_begin() }
        for tri in &self.pending_tris {
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
        self.pending_tris.clear();
    }

    /// Register a `C2D_SpriteSheet` loaded from the RomFS under a stable
    /// handle.
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

impl Default for N3dsRenderer {
    fn default() -> Self {
        Self::new()
    }
}

fn pack_color(c: Color) -> u32 {
    ((c.r as u32) << 24) | ((c.g as u32) << 16) | ((c.b as u32) << 8) | c.a as u32
}

impl Renderer for N3dsRenderer {
    fn caps(&self) -> &Caps {
        &self.caps
    }

    fn begin_frame(&mut self, clear: Color) {
        unsafe { ffi::trino_frame_begin(pack_color(clear)) }
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.flush_model_batch();
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
        self.flush_model_batch();
        self.camera = *camera;
    }

    fn draw_model(
        &mut self,
        model: ModelId,
        transform: &Transform3,
        _material: Material,
        params: &ModelParams,
    ) {
        let Some(tmdl) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_tmdl(tmdl).expect("validated on register");
        // Frustum clipping can fan one triangle into up to 6.
        let max_tris = mesh.index_count / 3 * 6;
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
            params.tint,
            Vec2::new(400.0, 240.0),
            &mut self.tri_scratch,
        );
        self.pending_tris.extend_from_slice(&self.tri_scratch[..n]);
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        unsafe { ffi::trino_frame_end() }
    }
}
