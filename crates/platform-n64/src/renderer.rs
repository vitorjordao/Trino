//! RDP sprite renderer via the shim's `rdpq` wrappers, plus gouraud
//! triangles for the engine's software-T&L 3D (`trino_core::render3d`).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::perf::RenderProfile;
use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, Mesh, MeshLayout, NEAR_PLANE, ScreenTri, TnlContext, TnlScratch,
    TnlStats, painter_index, painter_key, rsp_eligible_mv, rsp_mesh_supported,
};
use trino_core::text::BitmapFont;
use trino_core::{
    BillboardParams, Caps, Color, FontId, FrameStats, Mat34, Material, ModelId, ModelParams,
    Renderer, SpriteId, SpriteParams, TextParams, Transform3, Vec2, Vec3,
};

use crate::ffi;
use crate::rsp::{self, RspMesh};
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
    /// RSP copies of the meshes the overlay can draw (`rsp.rs`).
    rsp_meshes: BTreeMap<u32, RspMesh>,
    /// Send eligible draws to the RSP overlay (`set_rsp`; on by default).
    rsp_on: bool,
    /// RSP draw commands of the pending batch: submitted right after the
    /// batch's CPU triangles, so on equal depth they win like the last
    /// drawn surfaces of a painter-sorted batch do (a door on a wall).
    pending_rsp: Vec<[u32; rsp::DRAW_WORDS]>,
    /// The overlay's cull mode for back faces: `RDPQ_Triangle_Send_Async`'s
    /// v0, where 0 culls Trino's back faces (its "front" is the opposite
    /// winding; 1 culled every front face, measured) and 2 culls nothing.
    rsp_cull: u32,
    /// Stream RSP draws to the overlay as they come (when no CPU triangle
    /// is pending in the batch) instead of at the batch flush: the RSP then
    /// rasterizes while the CPU is still issuing draws. Off by default:
    /// at equal depth an RSP draw no longer wins over a CPU triangle that
    /// comes later in the same batch (castle64's door-on-wall).
    rsp_stream: bool,
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
            rsp_meshes: BTreeMap::new(),
            rsp_on: true,
            pending_rsp: Vec::new(),
            rsp_cull: 0,
            rsp_stream: false,
        }
    }

    /// Depth-sort and rasterize the pending model batch (painter's order
    /// across every mesh of the batch). Batches with textured draws are
    /// grouped by texture (stable: depth order within a texture) so TMEM is
    /// loaded once per texture per batch.
    fn flush_model_batch(&mut self) {
        self.flush_cpu_batch();
        if !self.pending_rsp.is_empty() {
            let t0 = self.tick();
            for w in &self.pending_rsp {
                unsafe { ffi::trino_rsp_draw(w.as_ptr()) };
            }
            self.pending_rsp.clear();
            self.charge(t0, |s| &mut s.submit);
        }
    }

    /// The CPU-tessellated part of the batch: sort + submit.
    fn flush_cpu_batch(&mut self) {
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

    /// RSP overlay self-check: a frame with one fixed triangle drawn by
    /// `rsp_trino.S`, then the pixel at its center (`None` = unreadable).
    pub fn rsp_selftest(&mut self) -> Option<Color> {
        self.begin_frame(Color::BLACK);
        unsafe {
            ffi::trino_3d_begin();
            ffi::trino_rsp_test_tri();
        }
        self.end_frame();
        self.read_pixel(50, 50)
    }

    /// Register a TMDL mesh blob under a stable handle (the asset loader,
    /// or a test building meshes in code). `false` = invalid blob, not
    /// registered.
    pub fn register_mesh(&mut self, id: u32, tmdl: Vec<u8>) -> bool {
        let Ok(layout) = Mesh::from_tmdl(&tmdl).map(|m| m.layout(&tmdl)) else {
            return false;
        };
        let mesh = Mesh::from_layout(&tmdl, &layout);
        if rsp_mesh_supported(&mesh)
            && let Some(rm) = rsp::build(&mesh)
        {
            self.rsp_meshes.insert(id, rm);
        }
        self.meshes.insert(id, (tmdl, layout));
        true
    }

    /// See [`N64Renderer::set_rsp_stream`].
    pub fn rsp_stream(&self) -> bool {
        self.rsp_stream
    }

    /// Stream RSP draws as they are issued (when the batch holds no CPU
    /// triangles yet) so the RSP overlaps the CPU's remaining draw calls.
    /// For scenes whose draws are (nearly) all RSP-eligible and that don't
    /// rely on RSP-after-CPU ordering at equal depth.
    pub fn set_rsp_stream(&mut self, on: bool) {
        self.rsp_stream = on;
    }

    /// Send eligible static draws to the RSP T&L overlay (`rsp_trino.S`)
    /// instead of the CPU's `tessellate`.
    pub fn set_rsp(&mut self, on: bool) {
        self.rsp_on = on;
    }

    /// Override the overlay's back-face cull mode (tests).
    pub fn set_rsp_cull(&mut self, cull: u32) {
        self.rsp_cull = cull;
    }

    /// RSP oracle: draw `model` through the overlay with its screen
    /// vertices copied back, and compare them with the CPU path's
    /// projection and lighting. `Ok` carries the worst differences.
    pub fn rsp_oracle(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        params: &ModelParams,
    ) -> Result<alloc::string::String, alloc::string::String> {
        // A copy: the frame below needs `self` mutably (a test, not a hot path).
        let Some((tmdl, layout)) = self.meshes.get(&model.0).cloned() else {
            return Err("mesh not registered".into());
        };
        let mesh = Mesh::from_layout(&tmdl, &layout);
        let Some(rm) = self.rsp_meshes.get(&model.0) else {
            return Err("mesh has no RSP copy".into());
        };
        let mv = self.tnl_ctx.projection().0.mul(matrix);
        let Some(d) = rsp_eligible_mv(&self.tnl_ctx, &mesh, matrix, &mv, params) else {
            return Err("draw not RSP-eligible".into());
        };
        let seg0 = rm.seg0.clone();
        let n = seg0.len();
        let buf = unsafe { ffi::trino_alloc_uncached((n * 24) as u32) } as *mut u8;
        let phys = unsafe { ffi::trino_phys_addr(buf as *const c_void) };
        let cull = if d.cull_back { self.rsp_cull } else { 2 };
        let words = rsp::draw_words(&self.tnl_ctx, &mv, &d, params.tint, rm, cull, phys);
        self.begin_frame(Color::BLACK);
        unsafe { ffi::trino_rsp_draw(words.as_ptr()) };
        self.end_frame();
        unsafe { ffi::trino_drain() };
        let (view, focal, screen) = self.tnl_ctx.projection();
        let mv = view.mul(matrix);
        let (cx, cy) = (screen.x * 0.5, screen.y * 0.5);
        let (mut dxy, mut dz, mut dc) = (0i32, 0i32, 0i32);
        for (k, &g) in seg0.iter().enumerate() {
            let g = g as usize;
            let rd = |o: usize| unsafe { buf.add(k * 24 + o).read_volatile() };
            let rd16 = |o: usize| i16::from_be_bytes([rd(o), rd(o + 1)]) as i32;
            let p = mv.transform_point(mesh.position(g));
            let ex = ((cx + p.x * focal / p.z) * 4.0) as i32;
            let ey = ((cy - p.y * focal / p.z) * 4.0) as i32;
            let ez = ((1.0 - NEAR_PLANE / p.z) * 32767.0) as i32;
            dxy = dxy.max((rd16(0) - ex).abs()).max((rd16(2) - ey).abs());
            dz = dz.max((rd16(4) - ez).abs());
            let n = mesh.normal(g);
            let lit = n.dot(d.light);
            let k = d.ambient + (1.0 - d.ambient) * if lit > 0.0 { lit } else { 0.0 };
            let c = mesh.color(g);
            let t = params.tint;
            let want = [
                c.r as f32 * (t.r as f32 / 255.0) * k,
                c.g as f32 * (t.g as f32 / 255.0) * k,
                c.b as f32 * (t.b as f32 / 255.0) * k,
                c.a as f32 * (t.a as f32 / 255.0),
            ];
            for (ch, w) in want.iter().enumerate() {
                dc = dc.max((rd(8 + ch) as i32 - *w as i32).abs());
            }
        }
        let msg = alloc::format!("{n} vertices: xy {dxy}/4 px, z {dz}/32767, color {dc}/255");
        if dxy > 2 || dz > 64 || dc > 3 {
            Err(msg)
        } else {
            Ok(msg)
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

/// Draws per streamed RSP chunk ([`N64Renderer::set_rsp_stream`]).
const RSP_STREAM_CHUNK: usize = 8;

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
        // `view · model` once: the eligibility test and the command share it.
        let rsp = match self.rsp_meshes.get(&model.0) {
            Some(rm) if self.rsp_on => {
                let mv = self.tnl_ctx.projection().0.mul(matrix);
                rsp_eligible_mv(&self.tnl_ctx, &mesh, matrix, &mv, params).map(|d| (rm, d, mv))
            }
            _ => None,
        };
        if let Some((rm, d, mv)) = rsp {
            // The overlay transforms, lights and rasterizes the whole draw;
            // the command goes out with the batch (flush_model_batch).
            let t0 = self.tick();
            let cull = if d.cull_back { self.rsp_cull } else { 2 };
            let words = rsp::draw_words(&self.tnl_ctx, &mv, &d, params.tint, rm, cull, 0);
            self.pending_rsp.push(words);
            // Streaming: hand the overlay a chunk as soon as it is full (and
            // the batch holds no CPU triangles), so the RSP starts while the
            // CPU is still issuing draws. Single-draw streaming hung the RDP
            // in ares under heavy 2D traffic (see the n64-performance skill);
            // chunks keep the proven burst pattern.
            if self.rsp_stream
                && self.pending_tris.is_empty()
                && self.pending_rsp.len() >= RSP_STREAM_CHUNK
            {
                for w in &self.pending_rsp {
                    unsafe { ffi::trino_rsp_draw(w.as_ptr()) };
                }
                self.pending_rsp.clear();
            }
            let tnl = TnlStats {
                rsp_draws: 1,
                rsp_tris: (mesh.index_count / 3) as u32,
                ..TnlStats::default()
            };
            self.frame_stats.tnl.add(&tnl);
            if let Some(p) = &mut self.profile {
                p.record_model(model, now_us().wrapping_sub(t0), &tnl);
            }
            return;
        }
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
        // Profiling: charge the RSP/RDP work this frame queued to the frame
        // itself (with RSP T&L it is no longer CPU time).
        if self.profile.is_some() {
            unsafe { ffi::trino_drain() }
        }
        self.charge(t0, |s| &mut s.end);
    }

    fn fill_rect(&mut self, rect: trino_core::Rect, color: Color) {
        self.flush_model_batch();
        let t0 = self.tick();
        let pack = |a: f32, b: f32| {
            (((a + 0.5) as i32 as u32 & 0xFFFF) << 16) | ((b + 0.5) as i32 as u32 & 0xFFFF)
        };
        let (x, y) = (rect.pos.x, rect.pos.y);
        // Round the edges (not the size): adjacent rectangles stay seamless.
        let (x1, y1) = (
            (x + rect.size.x + 0.5) as i32,
            (y + rect.size.y + 0.5) as i32,
        );
        let (x0, y0) = ((x + 0.5) as i32, (y + 0.5) as i32);
        let wh = pack((x1 - x0) as f32, (y1 - y0) as f32);
        unsafe { ffi::trino_fill_rect(pack(x0 as f32, y0 as f32), wh, pack_color(color)) };
        self.frame_stats.sprites += 1;
        self.charge(t0, |s| &mut s.sprites);
    }

    fn draw_text(&mut self, font: FontId, pos: Vec2, text: &str, params: &TextParams) {
        let Some(table) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        let Ok(parsed) = BitmapFont::parse(&table) else {
            return;
        };
        let atlas = trino_core::text::atlas_of(font);
        // Fast path: integer scale and an atlas that fits TMEM whole — one
        // upload, then a texture rectangle per glyph.
        let k = params.scale as u32;
        if k >= 1
            && params.scale == k as f32
            && let Some(entry) = self.sprites.get(&atlas.0)
        {
            let ptr = entry.ptr;
            self.flush_model_batch();
            let t0 = self.tick();
            if unsafe { ffi::trino_text_begin(ptr, pack_color(params.tint)) } == 1 {
                let mut glyphs = 0u32;
                trino_core::text::layout(&parsed, pos, text, params, &mut |r, at| {
                    let xy = ((at.x as i32 as u32 & 0xFFFF) << 16) | (at.y as i32 as u32 & 0xFFFF);
                    let st = ((r.pos.x as u32) << 16) | r.pos.y as u32;
                    let wh = ((r.size.x as u32) << 16) | r.size.y as u32;
                    unsafe { ffi::trino_text_glyph(xy, st, wh, k) };
                    glyphs += 1;
                });
                self.frame_stats.sprites += glyphs;
                self.charge(t0, |s| &mut s.sprites);
                return;
            }
            self.charge(t0, |s| &mut s.sprites);
        }
        trino_core::text::draw(self, atlas, &parsed, pos, text, params);
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
