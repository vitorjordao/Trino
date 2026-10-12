//! RDP sprite renderer via the shim's `rdpq` wrappers, plus gouraud
//! triangles for the engine's software-T&L 3D (`trino_core::render3d`).

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::render3d::{Camera3, DEFAULT_LIGHT, Light, Mesh, ScreenTri};
use trino_core::text::BitmapFont;
use trino_core::{
    Caps, Color, FontId, FrameStats, Mat34, Material, ModelId, ModelParams, Renderer, SpriteId,
    SpriteParams, TextParams, Transform3, Vec2,
};

use crate::ffi;

struct SpriteEntry {
    ptr: *mut c_void,
}

pub struct N64Renderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    meshes: BTreeMap<u32, Vec<u8>>,
    /// Glyph tables by font handle (atlas = sprite with the same handle).
    fonts: BTreeMap<u32, Rc<[u8]>>,
    frame_stats: FrameStats,
    last_stats: FrameStats,
    /// Current model batch: consecutive `draw_model` calls depth-sort
    /// together; flushed on sprite draws, camera changes and `end_frame`.
    pending_tris: Vec<ScreenTri>,
    camera: Camera3,
    light: Light,
    caps: Caps,
}

impl N64Renderer {
    pub fn new() -> Self {
        N64Renderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            fonts: BTreeMap::new(),
            frame_stats: FrameStats::default(),
            last_stats: FrameStats::default(),
            pending_tris: Vec::new(),
            camera: Camera3::default(),
            light: DEFAULT_LIGHT,
            caps: Caps::N64,
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
        self.frame_stats.tris += self.pending_tris.len() as u32;
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
            unsafe { ffi::trino_tri(pts.as_ptr(), colors.as_ptr(), tri.z.as_ptr()) }
        }
        self.pending_tris.clear();
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

    /// Register a font's glyph table (its atlas is the sprite `id`).
    pub(crate) fn register_font(&mut self, id: u32, table: Vec<u8>) {
        if BitmapFont::parse(&table).is_ok() {
            self.fonts.insert(id, table.into());
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
        self.frame_stats = FrameStats::default();
        unsafe { ffi::trino_frame_begin(pack_color(clear)) }
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.flush_model_batch();
        let Some(entry) = self.sprites.get(&sprite.0) else {
            return;
        };
        self.frame_stats.sprites += 1;
        let (src_x, src_y, src_w, src_h) = match params.region {
            Some(r) => (
                r.pos.x as i32,
                r.pos.y as i32,
                (r.size.x as i32).max(1),
                (r.size.y as i32).max(1),
            ),
            None => (0, 0, 0, 0),
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
            src_x,
            src_y,
            src_w,
            src_h,
        };
        unsafe { ffi::trino_sprite_blit(entry.ptr, &blit) }
    }

    fn set_camera(&mut self, camera: &Camera3) {
        self.flush_model_batch();
        self.camera = *camera;
    }

    fn set_light(&mut self, light: &Light) {
        // Lighting is resolved into vertex colors at tessellation time, so
        // a change needs no batch flush.
        self.light = *light;
    }

    fn draw_model(
        &mut self,
        model: ModelId,
        transform: &Transform3,
        material: Material,
        params: &ModelParams,
    ) {
        self.draw_model_matrix(model, &transform.matrix(), material, params);
    }

    fn draw_model_matrix(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        _material: Material,
        params: &ModelParams,
    ) {
        let Some(tmdl) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_tmdl(tmdl).expect("validated on register");
        self.frame_stats.models += 1;
        let pending = &mut self.pending_tris;
        trino_core::render3d::tessellate(
            &mesh,
            matrix,
            &self.camera,
            &self.light,
            params,
            Vec2::new(320.0, 240.0),
            &mut |tri| pending.push(tri),
        );
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        self.last_stats = self.frame_stats;
        unsafe { ffi::trino_frame_end() }
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        let Some(table) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        if let Ok(parsed) = BitmapFont::parse(&table) {
            trino_core::text::draw(
                self,
                trino_core::text::atlas_of(font),
                &parsed,
                pos,
                text,
                params,
            );
        }
    }

    fn measure_text(&self, font: FontId, text: &str, scale: f32) -> Vec2 {
        self.fonts
            .get(&font.0)
            .and_then(|t| BitmapFont::parse(t).ok())
            .map_or(Vec2::ZERO, |f| f.measure(text, scale))
    }

    fn stats(&self) -> FrameStats {
        self.last_stats
    }
}
