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

/// citro2d layers by a per-draw z (larger = nearer, depth-tested). Bands:
/// sprites drawn before any 3D sit under the 3D range; sprites after the
/// first 3D flush (the HUD) sit above it.
const SPRITE_DEPTH_BG: f32 = 0.05;
const SPRITE_DEPTH_HUD: f32 = 0.9;
const TRI_DEPTH_BASE: f32 = 0.1;
const TRI_DEPTH_RANGE: f32 = 0.35;

pub struct N3dsRenderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    meshes: BTreeMap<u32, Vec<u8>>,
    /// Current model batch: consecutive `draw_model` calls depth-sort
    /// together; flushed on sprite draws, camera changes and `end_frame`.
    pending_tris: Vec<ScreenTri>,
    /// Depth for the next sprite draw (background before 3D, HUD after).
    sprite_depth: f32,
    camera: Camera3,
    caps: Caps,
}

impl N3dsRenderer {
    pub fn new() -> Self {
        N3dsRenderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            pending_tris: Vec::new(),
            sprite_depth: SPRITE_DEPTH_BG,
            camera: Camera3::default(),
            caps: Caps::N3DS,
        }
    }

    /// Depth-sort (painter tie-break) and rasterize the pending model batch;
    /// each triangle carries a citro2d depth from its view-space nearness so
    /// the GPU depth test resolves overlap per pixel-ish (per triangle).
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
            // z normalizado (0 perto..1 longe) -> nearness na banda 3D.
            let avg = (tri.z[0] + tri.z[1] + tri.z[2]) * (1.0 / 3.0);
            let depth = TRI_DEPTH_BASE + TRI_DEPTH_RANGE * (1.0 - avg).clamp(0.0, 1.0);
            unsafe { ffi::trino_tri(pts.as_ptr(), colors.as_ptr(), depth) }
        }
        self.pending_tris.clear();
        // Sprites depois do 3D (HUD) ficam por cima da banda 3D.
        self.sprite_depth = SPRITE_DEPTH_HUD;
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
        self.sprite_depth = SPRITE_DEPTH_BG;
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
            depth: self.sprite_depth,
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
        let pending = &mut self.pending_tris;
        trino_core::render3d::tessellate(
            &mesh,
            &transform.matrix(),
            &self.camera,
            &DEFAULT_LIGHT,
            params.tint,
            Vec2::new(400.0, 240.0),
            &mut |tri| pending.push(tri),
        );
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        unsafe { ffi::trino_frame_end() }
    }
}
