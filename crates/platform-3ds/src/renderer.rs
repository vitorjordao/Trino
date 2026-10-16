//! citro2d sprite renderer (top screen 400x240, touch screen 320x240) via
//! the shim, plus gouraud triangles for the engine's software-T&L 3D
//! (`trino_core::render3d`), projected to whichever screen is current.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, MeshLayout, ScreenTri, TexBind, TnlContext, TnlScratch,
    painter_index, painter_key,
};
use trino_core::text::{BitmapFont, GlyphIndex};
use trino_core::{
    BillboardParams, Caps, Color, FontId, FrameStats, LightList, LocalLight, Mat34, Material,
    ModelId, ModelParams, Renderer, Screen, SpriteId, SpriteParams, TexFilter, TextParams,
    Transform3, Vec2, Vec3,
};

use crate::ffi;

/// The top screen: the game's picture (what the T&L projects to there).
const SCREEN: Vec2 = Vec2::new(400.0, 240.0);
/// The touch screen (`set_screen(1)`).
const BOTTOM: Vec2 = Vec2::new(320.0, 240.0);

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

/// A registered font: its glyph table and the direct index of its
/// codepoints below 256 (built once, at register).
struct FontEntry {
    table: Box<[u8]>,
    index: GlyphIndex,
}

impl FontEntry {
    fn font(&self) -> Option<BitmapFont<'_>> {
        BitmapFont::parse(&self.table)
            .ok()
            .map(|f| f.with_index(&self.index))
    }
}

pub struct N3dsRenderer {
    sprites: BTreeMap<u32, SpriteEntry>,
    /// TMDL blobs with their validated layout (parsed once, at register).
    meshes: BTreeMap<u32, (Vec<u8>, MeshLayout)>,
    /// Glyph tables by font handle (atlas = sprite with the same handle).
    fonts: BTreeMap<u32, Rc<FontEntry>>,
    frame_stats: FrameStats,
    last_stats: FrameStats,
    /// Current model batch: consecutive `draw_model` calls depth-sort
    /// together; flushed on sprite draws, camera changes and `end_frame`.
    pending_tris: Vec<ScreenTri>,
    /// Texture per pending triangle (parallel; filled once a textured draw
    /// joins the batch).
    /// (sprite handle, clamp) — clamp = billboard addressing.
    pending_tex: Vec<Option<TexBind>>,
    /// What `TexFilter::Default` means (`set_texture_filter`).
    tex_filter: TexFilter,
    /// Draw order of the pending batch: (texture group, painter key).
    order: Vec<(u64, u64)>,
    /// Vertex staging for one texture run (`trino_3d_draw` layout).
    verts: Vec<f32>,
    /// Depth for the next sprite draw (background before 3D, HUD after).
    sprite_depth: f32,
    /// 0 = top screen, 1 = touch screen (where draws go now).
    screen: u8,
    camera: Camera3,
    light: Light,
    /// Local lights of this frame (per vertex, `Caps::N3DS` cap).
    locals: LightList,
    /// T&L constants of `camera` + `light` + `locals`, rebuilt when one
    /// changes.
    tnl_ctx: TnlContext,
    /// The T&L's working memory, reused by every draw (boxed: ~9 KB).
    tnl_scratch: Box<TnlScratch>,
    caps: Caps,
}

impl N3dsRenderer {
    pub fn new() -> Self {
        N3dsRenderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            fonts: BTreeMap::new(),
            frame_stats: FrameStats::default(),
            last_stats: FrameStats::default(),
            pending_tris: Vec::new(),
            pending_tex: Vec::new(),
            tex_filter: TexFilter::Bilinear,
            order: Vec::new(),
            verts: Vec::new(),
            sprite_depth: SPRITE_DEPTH_BG,
            screen: 0,
            camera: Camera3::default(),
            tnl_ctx: TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN),
            tnl_scratch: Box::new(TnlScratch::new()),
            light: DEFAULT_LIGHT,
            locals: LightList::EMPTY,
            caps: Caps::N3DS,
        }
    }

    /// The current screen's size: what the T&L projects to.
    fn screen_px(&self) -> Vec2 {
        if self.screen == 1 { BOTTOM } else { SCREEN }
    }

    /// Rebuild the T&L constants for the camera, light, local lights and
    /// current screen.
    fn rebuild_tnl(&mut self) {
        self.tnl_ctx = TnlContext::new(&self.camera, &self.light, self.screen_px())
            .with_local_lights(self.locals.as_slice());
    }

    /// Depth-sort (painter tie-break) and rasterize the pending model batch.
    /// Every triangle goes through the citro3d path with a depth per vertex
    /// (from `ScreenTri::z`), so the GPU depth test resolves overlap per
    /// pixel — never citro2d's one-depth-per-triangle `C2D_DrawTriangle`.
    /// Triangles are grouped by texture (painter order within a group), one
    /// `trino_3d_draw` per run.
    fn flush_model_batch(&mut self) {
        if self.pending_tris.is_empty() {
            return;
        }
        self.frame_stats.tris += self.pending_tris.len() as u32;
        // Painter order as 8-byte keys (texture group, depth, submission
        // index) — never move the 88-byte triangles themselves.
        self.order.clear();
        for (i, tri) in self.pending_tris.iter().enumerate() {
            let group = match self.pending_tex.get(i).copied().flatten() {
                Some(t) => t.group(),
                None => 0,
            };
            self.order.push((group, painter_key(tri.depth, i)));
        }
        self.order.sort_unstable();
        let mut run: Option<TexBind> = None;
        self.verts.clear();
        for k in 0..self.order.len() {
            let i = painter_index(self.order[k].1);
            // A texture that never loaded draws untextured.
            let tex = self
                .pending_tex
                .get(i)
                .copied()
                .flatten()
                .filter(|t| self.sprites.contains_key(&t.id));
            if tex != run {
                self.draw_run(run);
                run = tex;
            }
            push_tri(&mut self.verts, &self.pending_tris[i], tex.is_some());
        }
        self.draw_run(run);
        self.pending_tris.clear();
        self.pending_tex.clear();
        // Sprites after the 3D (HUD) sit above the 3D band.
        self.sprite_depth = SPRITE_DEPTH_HUD;
    }

    /// Hand the staged vertices to the shim as one citro3d draw.
    fn draw_run(&mut self, tex: Option<TexBind>) {
        if self.verts.is_empty() {
            return;
        }
        let (sheet, clamp) = match tex.and_then(|t| self.sprites.get(&t.id).map(|e| (e, t))) {
            Some((entry, t)) => {
                let detail = t
                    .detail
                    .and_then(|d| self.sprites.get(&d.sprite.0).map(|e| (e.ptr, d)));
                let flags = ((t.filter == TexFilter::Point) as u32) << 1
                    | detail.map_or(0, |(_, d)| (d.scale_log2 as u32) << 4);
                let dptr = detail.map_or(core::ptr::null_mut(), |(p, _)| p);
                unsafe { ffi::trino_3d_material(dptr, flags) };
                (entry.ptr, t.clamp as u32)
            }
            None => (core::ptr::null_mut(), 0),
        };
        let count = (self.verts.len() / VERT_FLOATS) as u32;
        unsafe { ffi::trino_3d_draw(self.verts.as_ptr(), count, sheet, clamp) }
        self.verts.clear();
    }

    /// Test hook: print the top screen over the debug channel as
    /// `TRINO_FBR:` hex rows — `cargo xtask test 3ds` turns them into
    /// `target/3ds/castle64_hub.png` and checks it against the golden. Draw
    /// the frame a few times first (the LCD is double-buffered).
    pub fn dump_framebuffer(&mut self) {
        unsafe { ffi::trino_debug_fb_dump(0) }
    }

    /// The touch screen, the same way (the next golden after the top's).
    pub fn dump_framebuffer_bottom(&mut self) {
        unsafe { ffi::trino_debug_fb_dump(1) }
    }

    /// Register a `C2D_SpriteSheet` loaded from the RomFS under a stable
    /// handle.
    pub(crate) fn register(&mut self, id: u32, ptr: *mut c_void) {
        self.sprites.insert(id, SpriteEntry { ptr });
    }

    /// Record the texture of the `added` triangles appended at `before`
    /// (the tag list is only materialized once a textured draw joins).
    fn tag_pending(&mut self, before: usize, added: usize, key: Option<TexBind>) {
        if key.is_some() || !self.pending_tex.is_empty() {
            self.pending_tex.resize(before, None);
            self.pending_tex.extend(core::iter::repeat_n(key, added));
        }
    }

    /// Register a TMDL mesh blob under a stable handle.
    pub(crate) fn register_mesh(&mut self, id: u32, tmdl: Vec<u8>) {
        if let Ok(layout) = Mesh::from_tmdl(&tmdl).map(|m| m.layout(&tmdl)) {
            self.meshes.insert(id, (tmdl, layout));
        }
    }

    /// Register a font's glyph table (its atlas is the sprite `id`).
    pub(crate) fn register_font(&mut self, id: u32, table: Vec<u8>) {
        if let Ok(font) = BitmapFont::parse(&table) {
            let index = GlyphIndex::build(&font);
            let table = table.into_boxed_slice();
            self.fonts.insert(id, Rc::new(FontEntry { table, index }));
        }
    }
}

impl Default for N3dsRenderer {
    fn default() -> Self {
        Self::new()
    }
}

/// Normalized depth (0 near .. 1 far) -> nearness inside the 3D band.
fn band_depth(z: f32) -> f32 {
    TRI_DEPTH_BASE + TRI_DEPTH_RANGE * (1.0 - z).clamp(0.0, 1.0)
}

/// Floats per vertex in the `trino_3d_draw` layout.
const VERT_FLOATS: usize = 10;

/// Append one triangle in the `trino_3d_draw` layout: per vertex x, y,
/// depth (per vertex — the point of the citro3d path), w, u, v, r, g, b, a.
/// Untextured triangles pass w = 1: colors then interpolate screen-linearly,
/// like the PC and N64 rasterizers (and citro2d before); textured ones pass
/// the view depth for perspective-correct UVs.
fn push_tri(out: &mut Vec<f32>, tri: &ScreenTri, textured: bool) {
    for i in 0..3 {
        let c = tri.colors[i];
        out.extend_from_slice(&[
            tri.pts[i].x,
            tri.pts[i].y,
            band_depth(tri.z[i]),
            if textured { tri.w[i] } else { 1.0 },
            tri.uv[i].x,
            tri.uv[i].y,
            c.r as f32 / 255.0,
            c.g as f32 / 255.0,
            c.b as f32 / 255.0,
            c.a as f32 / 255.0,
        ]);
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
        self.frame_stats = FrameStats::default();
        // The last frame may have ended on the touch screen (project to the
        // top again) or with local lights (cleared every frame).
        let was_bottom = self.screen != 0;
        self.screen = 0;
        let had_lights = self.locals.set(&[], 0);
        if was_bottom || had_lights {
            self.rebuild_tnl();
        }
        unsafe { ffi::trino_frame_begin(pack_color(clear)) }
    }

    fn second_screen(&self) -> Option<Vec2> {
        Some(BOTTOM)
    }

    fn set_screen(&mut self, screen: u8) {
        // The top screen first, then the touch screen, once per frame
        // (like the PC backend, which renders the top on the switch).
        if screen == 1 && self.screen == 0 {
            self.flush_model_batch();
            self.screen = 1;
            // A fresh scene: 2D depth starts over on the other screen, and
            // 3D projects to its size (the shim swaps the projection).
            self.sprite_depth = SPRITE_DEPTH_BG;
            self.rebuild_tnl();
            unsafe { ffi::trino_set_screen(1) }
        }
    }

    fn screen(&self) -> Screen {
        if self.screen == 1 {
            Screen::Bottom
        } else {
            Screen::Top
        }
    }

    fn fill_rect(&mut self, rect: trino_core::Rect, color: Color) {
        self.flush_model_batch();
        self.frame_stats.sprites += 1;
        let f = ffi::TrinoFill {
            x: rect.pos.x,
            y: rect.pos.y,
            w: rect.size.x,
            h: rect.size.y,
            depth: self.sprite_depth,
            color: pack_color(color),
        };
        unsafe { ffi::trino_fill_rect(&f) }
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
            depth: self.sprite_depth,
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
        self.rebuild_tnl();
    }

    fn set_light(&mut self, light: &Light) {
        // Lighting is resolved into vertex colors at tessellation time, so
        // a change needs no batch flush.
        self.light = *light;
        self.rebuild_tnl();
    }

    fn set_texture_filter(&mut self, filter: TexFilter) {
        self.tex_filter = match filter {
            TexFilter::Default => TexFilter::Bilinear,
            f => f,
        };
    }

    fn set_local_lights(&mut self, lights: &[LocalLight]) {
        // Per vertex, like the global light: no flush.
        if self.locals.set(lights, self.caps.max_local_lights) {
            self.rebuild_tnl();
        }
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
        let added = self.pending_tris.len() - before;
        let filter = self.tex_filter;
        self.tag_pending(
            before,
            added,
            texture.map(|t| TexBind::model(t.0, params, filter)),
        );
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
        self.tag_pending(
            before,
            2,
            Some(TexBind::billboard(sprite.0, self.tex_filter)),
        );
    }

    fn end_frame(&mut self) {
        self.flush_model_batch();
        self.last_stats = self.frame_stats;
        unsafe { ffi::trino_frame_end() }
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        let Some(entry) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        if let Some(parsed) = entry.font() {
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
            .and_then(|e| e.font())
            .map_or(Vec2::ZERO, |f| f.measure(text, scale))
    }

    fn stats(&self) -> FrameStats {
        self.last_stats
    }

    fn read_pixel(&mut self, x: u32, y: u32) -> Option<Color> {
        self.read_screen_pixel(Screen::Top, x, y)
    }

    fn read_screen_pixel(&mut self, screen: Screen, x: u32, y: u32) -> Option<Color> {
        let c = unsafe { ffi::trino_debug_pixel(x, y, screen.index() as u32) };
        (c != 0).then(|| Color::rgba((c >> 24) as u8, (c >> 16) as u8, (c >> 8) as u8, c as u8))
    }
}
