//! RDP sprite renderer via the shim's `rdpq` wrappers, plus gouraud
//! triangles for the engine's software-T&L 3D (`trino_core::render3d`).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::perf::RenderProfile;
use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, MeshLayout, ScreenTri, TnlContext, TnlScratch,
    painter_index, painter_key,
};
use trino_core::text::BitmapFont;
use trino_core::{
    BillboardParams, Caps, Color, FontId, FrameStats, Mat34, Material, ModelId, ModelParams,
    Renderer, SpriteId, SpriteParams, TextParams, Transform3, Vec2, Vec3,
};

use crate::ffi;
use crate::runtime::now_us;

/// Internal resolution the T&L projects to.
const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

struct SpriteEntry {
    ptr: *mut c_void,
}

pub struct N64Renderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    /// TMDL blobs with their validated layout (parsed once, at register).
    meshes: BTreeMap<u32, (Vec<u8>, MeshLayout)>,
    /// Glyph tables by font handle (atlas = sprite with the same handle).
    fonts: BTreeMap<u32, Rc<[u8]>>,
    frame_stats: FrameStats,
    last_stats: FrameStats,
    /// Current model batch: consecutive `draw_model` calls depth-sort
    /// together; flushed on sprite draws, camera changes and `end_frame`.
    pending_tris: Vec<ScreenTri>,
    /// Texture per pending triangle (parallel; filled once a textured draw
    /// joins the batch).
    /// (sprite handle, clamp) — clamp = billboard addressing.
    pending_tex: Vec<Option<(u32, bool)>>,
    /// Draw order of the pending batch: (texture group, painter key).
    order: Vec<(u64, u64)>,
    camera: Camera3,
    light: Light,
    /// T&L constants of `camera` + `light`, rebuilt when either changes.
    tnl_ctx: TnlContext,
    /// The T&L's working memory, reused by every draw (boxed: ~9 KB).
    tnl_scratch: Box<TnlScratch>,
    caps: Caps,
    /// Stage/model timing on the console clock while profiling is on.
    profile: Option<RenderProfile>,
}

impl N64Renderer {
    /// Start timing every stage and model (see `trino_core::perf`).
    pub fn start_profile(&mut self) {
        self.profile = Some(RenderProfile::default());
    }

    /// Stop profiling and hand back what was measured.
    pub fn take_profile(&mut self) -> Option<RenderProfile> {
        self.profile.take()
    }

    /// Timestamp when profiling (no timer reads otherwise).
    fn tick(&self) -> u32 {
        if self.profile.is_some() { now_us() } else { 0 }
    }

    /// Add the microseconds since `t0` to one stage.
    fn charge(&mut self, t0: u32, stage: impl Fn(&mut trino_core::perf::StageTimes) -> &mut u32) {
        if let Some(p) = &mut self.profile {
            *stage(&mut p.stages) += now_us().wrapping_sub(t0);
        }
    }

    /// Test hook: print the last presented framebuffer over ISViewer as
    /// `TRINO_FBR:` hex rows — `cargo xtask test n64` turns them into the
    /// emulator golden image (`tests/golden/n64/`).
    pub fn dump_framebuffer(&mut self) {
        unsafe { ffi::trino_debug_fb_dump() }
    }

    pub fn new() -> Self {
        N64Renderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            fonts: BTreeMap::new(),
            frame_stats: FrameStats::default(),
            last_stats: FrameStats::default(),
            pending_tris: Vec::new(),
            pending_tex: Vec::new(),
            order: Vec::new(),
            camera: Camera3::default(),
            tnl_ctx: TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN),
            tnl_scratch: Box::new(TnlScratch::new()),
            light: DEFAULT_LIGHT,
            caps: Caps::N64,
            profile: None,
        }
    }

    /// Depth-sort and rasterize the pending model batch (painter's order
    /// across every mesh of the batch). Batches with textured draws are
    /// grouped by texture (stable: depth order within a texture) so TMEM is
    /// loaded once per texture per batch.
    fn flush_model_batch(&mut self) {
        if self.pending_tris.is_empty() {
            return;
        }
        self.frame_stats.tris += self.pending_tris.len() as u32;
        let textured = self.pending_tex.iter().any(Option::is_some);
        let t0 = self.tick();
        // Painter order as 8-byte keys (texture group, depth, submission
        // index) — never move the 88-byte triangles themselves.
        self.order.clear();
        for (i, tri) in self.pending_tris.iter().enumerate() {
            let group = match self.pending_tex.get(i).copied().flatten() {
                Some((id, clamp)) => (((id as u64) << 1) | clamp as u64) + 1,
                None => 0,
            };
            self.order.push((group, painter_key(tri.depth, i)));
        }
        self.order.sort_unstable();
        self.charge(t0, |s| &mut s.sort);
        let t1 = self.tick();
        if !textured {
            unsafe { ffi::trino_3d_begin() }
            for &(_, key) in &self.order {
                emit_tri(&self.pending_tris[painter_index(key)]);
            }
        } else {
            let mut bound: Option<Option<(u32, bool)>> = None;
            for &(_, key) in &self.order {
                let i = painter_index(key);
                let tex = self.pending_tex.get(i).copied().flatten();
                if bound != Some(tex) {
                    match tex.and_then(|(t, clamp)| self.sprites.get(&t).map(|e| (e, clamp))) {
                        Some((entry, clamp)) => unsafe {
                            ffi::trino_3d_texture(entry.ptr, clamp as u32)
                        },
                        None => unsafe { ffi::trino_3d_begin() },
                    }
                    bound = Some(tex);
                }
                if tex.is_some_and(|(t, _)| self.sprites.contains_key(&t)) {
                    emit_tri_tex(&self.pending_tris[i]);
                } else {
                    emit_tri(&self.pending_tris[i]);
                }
            }
        }
        self.charge(t1, |s| &mut s.submit);
        self.pending_tris.clear();
        self.pending_tex.clear();
    }

    /// One RDP sprite blit (no batch flush).
    fn blit(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
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

    /// Register a `sprite_t` loaded from the DFS under a stable handle.
    pub(crate) fn register(&mut self, id: u32, ptr: *mut c_void) {
        self.sprites.insert(id, SpriteEntry { ptr });
    }

    /// Record the texture of the `added` triangles appended at `before`
    /// (the tag list is only materialized once a textured draw joins).
    fn tag_pending(&mut self, before: usize, added: usize, key: Option<(u32, bool)>) {
        if key.is_some() || !self.pending_tex.is_empty() {
            self.pending_tex.resize(before, None);
            self.pending_tex.extend(core::iter::repeat_n(key, added));
        }
    }

    /// Register a TMDL mesh blob under a stable handle (the asset loader,
    /// or a test building meshes in code). `false` = invalid blob, not
    /// registered.
    pub fn register_mesh(&mut self, id: u32, tmdl: Vec<u8>) -> bool {
        let Ok(layout) = Mesh::from_tmdl(&tmdl).map(|m| m.layout(&tmdl)) else {
            return false;
        };
        self.meshes.insert(id, (tmdl, layout));
        true
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

/// Gouraud triangle (vertex colors only).
fn emit_tri(tri: &ScreenTri) {
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

/// Textured + gouraud triangle (texture bound by `trino_3d_texture`).
fn emit_tri_tex(tri: &ScreenTri) {
    let mut v = [0f32; 30];
    for i in 0..3 {
        let c = tri.colors[i];
        v[i * 10..i * 10 + 10].copy_from_slice(&[
            tri.pts[i].x,
            tri.pts[i].y,
            tri.z[i],
            tri.uv[i].x,
            tri.uv[i].y,
            tri.w[i],
            c.r as f32 / 255.0,
            c.g as f32 / 255.0,
            c.b as f32 / 255.0,
            c.a as f32 / 255.0,
        ]);
    }
    unsafe { ffi::trino_tri_tex(v.as_ptr()) }
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
        let t0 = self.tick();
        unsafe { ffi::trino_frame_begin(pack_color(clear)) }
        self.charge(t0, |s| &mut s.begin);
        if let Some(p) = &mut self.profile {
            p.frames += 1;
        }
    }

    fn draw_sprite(&mut self, sprite: SpriteId, pos: Vec2, params: &SpriteParams) {
        self.flush_model_batch();
        let t0 = self.tick();
        self.blit(sprite, pos, params);
        self.charge(t0, |s| &mut s.sprites);
    }

    fn set_camera(&mut self, camera: &Camera3) {
        self.flush_model_batch();
        self.camera = *camera;
        self.tnl_ctx = TnlContext::new(&self.camera, &self.light, SCREEN);
    }

    fn set_light(&mut self, light: &Light) {
        // Lighting is resolved into vertex colors at tessellation time, so
        // a change needs no batch flush.
        self.light = *light;
        self.tnl_ctx = TnlContext::new(&self.camera, &self.light, SCREEN);
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
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        self.frame_stats.models += 1;
        let texture = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0));
        let before = self.pending_tris.len();
        let t0 = self.tick();
        let pending = &mut self.pending_tris;
        let tnl = trino_core::render3d::tessellate_with(
            &self.tnl_ctx,
            &mut self.tnl_scratch,
            &mesh,
            matrix,
            params,
            &mut |tri| pending.push(tri),
        );
        self.frame_stats.tnl.add(&tnl);
        if let Some(p) = &mut self.profile {
            p.record_model(model, now_us().wrapping_sub(t0), &tnl);
        }
        let added = self.pending_tris.len() - before;
        self.tag_pending(before, added, texture.map(|t| (t.0, false)));
    }

    fn draw_billboard(&mut self, sprite: SpriteId, pos: Vec3, params: &BillboardParams) {
        let Some(entry) = self.sprites.get(&sprite.0) else {
            return;
        };
        let size = unsafe { ffi::trino_sprite_size(entry.ptr) };
        let (w, h) = ((size >> 16) as f32, (size & 0xFFFF) as f32);
        let uv = trino_core::render3d::region_uv(params.region, w, h);
        let Some(tris) = trino_core::render3d::billboard_with(&self.tnl_ctx, pos, params, uv)
        else {
            return;
        };
        self.frame_stats.sprites += 1;
        let before = self.pending_tris.len();
        self.pending_tris.extend_from_slice(&tris);
        self.tag_pending(before, 2, Some((sprite.0, true)));
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        self.last_stats = self.frame_stats;
        let t0 = self.tick();
        unsafe { ffi::trino_frame_end() }
        self.charge(t0, |s| &mut s.end);
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

    fn read_pixel(&mut self, x: u32, y: u32) -> Option<Color> {
        let c = unsafe { ffi::trino_debug_pixel(x, y) };
        (c != 0).then(|| Color::rgba((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8, c as u8))
    }
}
