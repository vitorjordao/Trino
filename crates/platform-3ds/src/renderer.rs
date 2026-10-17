//! citro2d sprite renderer (top screen 400x240, touch screen 320x240) via
//! the shim, plus gouraud triangles for the engine's software-T&L 3D
//! (`trino_core::render3d`), projected to whichever screen is current.

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ffi::c_void;

use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, GpuReject, GpuWater, Light, Mesh, MeshLayout, ScreenTri, TexBind,
    TnlContext, TnlScratch, TnlStats, gpu_check_mv, gpu_mesh_check, gpu_screen_matrix, gpu_water,
    painter_index, painter_key,
};
use trino_core::text::{BitmapFont, GlyphIndex};
use trino_core::{
    BillboardParams, Caps, Color, FontId, FrameStats, LightList, LocalLight, Mat34, Material,
    ModelId, ModelParams, Renderer, Ripple, Screen, SpriteId, SpriteParams, TexFilter, TextParams,
    Transform3, Vec2, Vec3, WaterState,
};

use crate::ffi;

/// The top screen: the game's picture (what the T&L projects to there).
const SCREEN: Vec2 = Vec2::new(400.0, 240.0);
/// The touch screen (`set_screen(1)`).
const BOTTOM: Vec2 = Vec2::new(320.0, 240.0);
/// Bytes per vertex of a GPU T&L mesh (the shim's `TNL_VERTEX_BYTES`).
const GPU_VERTEX_BYTES: usize = 28;
/// Floats of uniforms per GPU T&L draw (`trino_gpu_draw`): the screen
/// matrix, then light, ambient, diffuse, tint, emissive, fog row and fog
/// color.
const GPU_UNIFORM_FLOATS: usize = 44;

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
    /// The GPU T&L copies of the meshes that can take that path
    /// ([`gpu_mesh_check`]): shim handle and triangle count.
    gpu_meshes: BTreeMap<u32, (*mut c_void, u32)>,
    /// Why each other mesh can never take it.
    gpu_mesh_rejects: BTreeMap<u32, GpuReject>,
    /// This frame's and the last frame's model draws left on the CPU, by
    /// reason ([`GpuReject::index`]).
    gpu_rejects: [u16; GpuReject::COUNT],
    last_gpu_rejects: [u16; GpuReject::COUNT],
    /// GPU T&L on (the default); off draws everything through the CPU's
    /// software T&L (`set_gpu_tnl`, the self-test's reference).
    gpu_on: bool,
    /// Microseconds spent waiting in `begin_frame` (the previous frame's
    /// GPU work and vsync) since the last [`take_wait_us`](Self::take_wait_us).
    wait_us: u32,
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
    /// This frame's water clock and ripples (`set_water`, cleared at
    /// `begin_frame`).
    water_state: WaterState,
    /// The batch's water triangles and what each samples (with its detail
    /// layer's scroll): drawn after the batch, far to near, blended without
    /// writing depth.
    pending_water: Vec<ScreenTri>,
    pending_water_tex: Vec<Option<TexBind>>,
    /// Water meshes copied for the GPU water program (`water.v.pica`), by
    /// model, made on their first water draw (null: no linear memory).
    gpu_water_meshes: BTreeMap<u32, *mut c_void>,
    /// The batch's GPU water draws: drawn after its CPU triangles, before
    /// the CPU water (the ripple rings ride on top).
    pending_gpu_water: Vec<GpuWaterDraw>,
}

/// One queued GPU water draw.
struct GpuWaterDraw {
    mesh: *mut c_void,
    uniforms: [f32; WATER_UNIFORM_FLOATS],
    texture: Option<TexBind>,
    cull: u32,
}

/// Floats of uniforms per GPU water draw (`trino_water_draw`).
const WATER_UNIFORM_FLOATS: usize = 104;

impl N3dsRenderer {
    pub fn new() -> Self {
        N3dsRenderer {
            sprites: BTreeMap::new(),
            meshes: BTreeMap::new(),
            gpu_meshes: BTreeMap::new(),
            gpu_mesh_rejects: BTreeMap::new(),
            gpu_rejects: [0; GpuReject::COUNT],
            last_gpu_rejects: [0; GpuReject::COUNT],
            gpu_on: true,
            wait_us: 0,
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
            water_state: WaterState::NONE,
            pending_water: Vec::new(),
            pending_water_tex: Vec::new(),
            gpu_water_meshes: BTreeMap::new(),
            pending_gpu_water: Vec::new(),
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
        self.flush_cpu_batch();
        self.flush_gpu_water();
        self.flush_water();
    }

    /// The batch's GPU water surfaces (`water.v.pica`).
    fn flush_gpu_water(&mut self) {
        for d in core::mem::take(&mut self.pending_gpu_water) {
            let sheet = match d
                .texture
                .and_then(|t| self.sprites.get(&t.id).map(|e| (e.ptr, t)))
            {
                Some((ptr, t)) => {
                    let detail = t
                        .detail
                        .and_then(|dt| self.sprites.get(&dt.sprite.0).map(|e| (e.ptr, dt)));
                    let flags = ((t.filter == TexFilter::Point) as u32) << 1
                        | detail.map_or(0, |(_, dt)| (dt.scale_log2 as u32) << 4)
                        | 1 << 8;
                    let dptr = detail.map_or(core::ptr::null_mut(), |(p, _)| p);
                    unsafe { ffi::trino_3d_material(dptr, flags, 0) };
                    ptr
                }
                None => {
                    unsafe { ffi::trino_3d_material(core::ptr::null_mut(), 1 << 8, 0) };
                    core::ptr::null_mut()
                }
            };
            unsafe { ffi::trino_water_draw(d.mesh, d.uniforms.as_ptr(), sheet, d.cull) };
            self.sprite_depth = SPRITE_DEPTH_HUD;
        }
    }

    /// The batch's water, last: far to near, one `trino_3d_draw` per
    /// surface (water material: blended, depth tested, not written).
    fn flush_water(&mut self) {
        if self.pending_water.is_empty() {
            return;
        }
        self.frame_stats.tris += self.pending_water.len() as u32;
        self.order.clear();
        for (i, tri) in self.pending_water.iter().enumerate() {
            let group = self.pending_water_tex[i].map_or(0, |t| t.group());
            self.order.push((group, painter_key(tri.depth, i)));
        }
        self.order.sort_unstable();
        let mut run: Option<Option<TexBind>> = None;
        self.verts.clear();
        for k in 0..self.order.len() {
            let i = painter_index(self.order[k].1);
            let tex = self.pending_water_tex[i].filter(|t| self.sprites.contains_key(&t.id));
            if run != Some(tex) {
                if let Some(prev) = run {
                    self.draw_run_with(prev, true);
                }
                run = Some(tex);
            }
            push_tri(&mut self.verts, &self.pending_water[i], tex.is_some());
        }
        if let Some(prev) = run {
            self.draw_run_with(prev, true);
        }
        self.pending_water.clear();
        self.pending_water_tex.clear();
        self.sprite_depth = SPRITE_DEPTH_HUD;
    }

    /// The CPU-tessellated part of the batch (not the water).
    fn flush_cpu_batch(&mut self) {
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
        self.draw_run_with(tex, false);
    }

    /// [`Self::draw_run`], as water (`water`: ADR 0015's material) or not.
    /// The material is set for every run, so water never leaks into the
    /// next draw.
    fn draw_run_with(&mut self, tex: Option<TexBind>, water: bool) {
        if self.verts.is_empty() {
            return;
        }
        let water_bit = (water as u32) << 8;
        let (sheet, clamp) = match tex.and_then(|t| self.sprites.get(&t.id).map(|e| (e, t))) {
            Some((entry, t)) => {
                let detail = t
                    .detail
                    .and_then(|d| self.sprites.get(&d.sprite.0).map(|e| (e.ptr, d)));
                let flags = ((t.filter == TexFilter::Point) as u32) << 1
                    | detail.map_or(0, |(_, d)| (d.scale_log2 as u32) << 4)
                    | water_bit;
                let dptr = detail.map_or(core::ptr::null_mut(), |(p, _)| p);
                let [u, v] = t.water.unwrap_or([0, 0]);
                unsafe { ffi::trino_3d_material(dptr, flags, (u as u32) << 16 | v as u32) };
                (entry.ptr, t.clamp as u32)
            }
            None => {
                unsafe { ffi::trino_3d_material(core::ptr::null_mut(), water_bit, 0) };
                (core::ptr::null_mut(), 0)
            }
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
        let Ok(mesh) = Mesh::from_tmdl(&tmdl) else {
            return;
        };
        let check = gpu_mesh_check(&mesh);
        if check.is_ok() {
            let mut verts = Vec::with_capacity(mesh.vertex_count * GPU_VERTEX_BYTES);
            for i in 0..mesh.vertex_count {
                let (p, n, c) = (mesh.position(i), mesh.normal(i), mesh.color(i));
                for f in [p.x, p.y, p.z, n.x, n.y, n.z] {
                    verts.extend_from_slice(&f.to_ne_bytes());
                }
                verts.extend_from_slice(&[c.r, c.g, c.b, c.a]);
            }
            let idx: Vec<u16> = (0..mesh.index_count).map(|i| mesh.index(i)).collect();
            let ptr = unsafe {
                ffi::trino_gpu_mesh(
                    verts.as_ptr(),
                    mesh.vertex_count as u32,
                    idx.as_ptr(),
                    idx.len() as u32,
                )
            };
            if ptr.is_null() {
                // Linear memory is short: this mesh keeps the CPU path.
                crate::runtime::log(&alloc::format!(
                    "TRINO_WARN: no linear memory for the GPU copy of model {id:08x} \
                     ({} vertices): CPU T&L\n",
                    mesh.vertex_count
                ));
                self.gpu_mesh_rejects.insert(id, GpuReject::NoGpuMemory);
            } else {
                self.gpu_meshes
                    .insert(id, (ptr, (mesh.index_count / 3) as u32));
            }
        } else if let Err(r) = check {
            self.gpu_mesh_rejects.insert(id, r);
        }
        let layout = mesh.layout(&tmdl);
        self.meshes.insert(id, (tmdl, layout));
    }

    /// The time `begin_frame` waited (GPU and vsync) since the last call:
    /// frame time minus it is the ARM11's own work.
    pub fn take_wait_us(&mut self) -> u32 {
        core::mem::take(&mut self.wait_us)
    }

    /// GPU T&L for the draws that can take it (on by default). Off, every
    /// draw runs the CPU's software T&L: the reference the self-test
    /// compares the GPU path with.
    pub fn set_gpu_tnl(&mut self, on: bool) {
        self.flush_model_batch();
        self.gpu_on = on;
    }

    /// The last frame's model draws left on the CPU T&L path, by reason
    /// ([`GpuReject::index`]; all zero with the GPU path off).
    pub fn gpu_rejects(&self) -> [u16; GpuReject::COUNT] {
        self.last_gpu_rejects
    }

    /// The GPU T&L draw of `model` if this draw may take that path: the
    /// shim's mesh, its triangles, the uniforms and the cull flag; else why
    /// not.
    fn gpu_plan(
        &self,
        model: ModelId,
        mesh: &Mesh,
        matrix: &Mat34,
        params: &ModelParams,
    ) -> Result<(*mut c_void, u32, [f32; GPU_UNIFORM_FLOATS], u32), GpuReject> {
        let Some(&(ptr, tris)) = self.gpu_meshes.get(&model.0) else {
            return Err(self
                .gpu_mesh_rejects
                .get(&model.0)
                .copied()
                .unwrap_or(GpuReject::Empty));
        };
        let mv = self.tnl_ctx.projection().0.mul(matrix);
        let d = gpu_check_mv(&self.tnl_ctx, mesh, matrix, &mv, params)?;
        let s = gpu_screen_matrix(&self.tnl_ctx, &mv);
        // The CPU path's depth band (`band_depth`) of the z-buffer depth
        // row, times the view depth like the rest.
        let (b, k) = (TRI_DEPTH_BASE + TRI_DEPTH_RANGE, TRI_DEPTH_RANGE);
        let band: [f32; 4] = core::array::from_fn(|j| b * s[3][j] - k * s[2][j]);
        let mut u = [0.0f32; GPU_UNIFORM_FLOATS];
        for (r, row) in [s[0], s[1], band, s[3]].iter().enumerate() {
            u[r * 4..r * 4 + 4].copy_from_slice(row);
        }
        u[16..19].copy_from_slice(&[d.light.x, d.light.y, d.light.z]);
        u[20..23].copy_from_slice(&d.ambient);
        u[24..27].copy_from_slice(&d.diffuse);
        u[28..32].copy_from_slice(&d.tint);
        u[32..35].copy_from_slice(&d.emissive);
        u[36..40].copy_from_slice(&d.fog);
        u[40..43].copy_from_slice(&d.fog_color);
        Ok((ptr, tris, u, d.cull_back as u32))
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

/// Copy a water mesh for `trino_water_draw` (24-byte vertices: position,
/// UV, color); null when linear memory is short.
fn upload_water_mesh(mesh: &Mesh) -> *mut c_void {
    let mut verts = Vec::with_capacity(mesh.vertex_count * 24);
    for i in 0..mesh.vertex_count {
        let (p, c) = (mesh.position(i), mesh.color(i));
        let uv = if mesh.has_uvs() {
            mesh.uv(i)
        } else {
            Vec2::ZERO
        };
        for f in [p.x, p.y, p.z, uv.x, uv.y] {
            verts.extend_from_slice(&f.to_ne_bytes());
        }
        verts.extend_from_slice(&[c.r, c.g, c.b, c.a]);
    }
    let idx: Vec<u16> = (0..mesh.index_count).map(|i| mesh.index(i)).collect();
    unsafe {
        ffi::trino_water_mesh(
            verts.as_ptr(),
            mesh.vertex_count as u32,
            idx.as_ptr(),
            idx.len() as u32,
        )
    }
}

/// [`GpuWater`] as `trino_water_draw`'s floats: the world-to-screen
/// matrix with the CPU path's depth band (as `gpu_plan` builds it), the
/// model rows, then the vectors in `water.v.pica`'s order.
fn water_uniforms(u: &GpuWater) -> [f32; WATER_UNIFORM_FLOATS] {
    let s = u.screen;
    let (b, k) = (TRI_DEPTH_BASE + TRI_DEPTH_RANGE, TRI_DEPTH_RANGE);
    let band: [f32; 4] = core::array::from_fn(|j| b * s[3][j] - k * s[2][j]);
    let mut out = [0.0f32; WATER_UNIFORM_FLOATS];
    let rows = [s[0], s[1], band, s[3], u.model[0], u.model[1], u.model[2]];
    let vecs = [
        u.wdir,
        u.wk,
        u.wamp,
        u.rx,
        u.rz,
        u.rrad,
        u.rstr,
        u.light,
        u.ambient,
        u.diffuse,
        u.tint,
        u.emissive,
        u.eye,
        u.sky,
        u.glint,
        u.fog,
        u.fog_color,
        u.scroll,
        u.detail_offset,
    ];
    for (i, v) in rows.iter().chain(vecs.iter()).enumerate() {
        out[i * 4..i * 4 + 4].copy_from_slice(v);
    }
    out
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
        self.water_state = WaterState::NONE;
        // The last frame may have ended on the touch screen (project to the
        // top again) or with local lights (cleared every frame).
        let was_bottom = self.screen != 0;
        self.screen = 0;
        let had_lights = self.locals.set(&[], 0);
        if was_bottom || had_lights {
            self.rebuild_tnl();
        }
        let t0 = crate::runtime::now_us();
        unsafe { ffi::trino_frame_begin(pack_color(clear)) }
        self.wait_us = self
            .wait_us
            .wrapping_add(crate::runtime::now_us().wrapping_sub(t0));
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

    fn set_water(&mut self, time: f32, ripples: &[Ripple]) {
        self.water_state = WaterState::new(time, ripples, self.caps.max_ripples);
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
        let plan = match self.gpu_on {
            true => match self.gpu_plan(model, &mesh, matrix, params) {
                Ok(plan) => Some(plan),
                Err(r) => {
                    let n = &mut self.gpu_rejects[r.index()];
                    *n = n.saturating_add(1);
                    None
                }
            },
            false => None,
        };
        if let Some((ptr, tris, u, cull)) = plan {
            unsafe { ffi::trino_gpu_draw(ptr, u.as_ptr(), cull) };
            self.frame_stats.tris += tris;
            self.frame_stats.tnl.add(&TnlStats {
                gpu_draws: 1,
                gpu_tris: tris,
                ..TnlStats::default()
            });
            // Sprites after 3D (the HUD) sit above the 3D band.
            self.sprite_depth = SPRITE_DEPTH_HUD;
            return;
        }
        let texture = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0));
        if let Some(w) = params.water {
            // Water: on the GPU's water program when it can take it (the
            // ripple rings stay on the CPU), else displaced, shaded and
            // ringed by the CPU T&L; drawn with the batch's water (after
            // the rest, blended).
            let ctx = self.tnl_ctx.with_water(&self.water_state);
            let time = self.water_state.time;
            let bind =
                texture.map(|t| TexBind::model(t.0, params, self.tex_filter).with_water(&w, time));
            let scale = bind
                .and_then(|t| t.detail)
                .filter(|d| self.sprites.contains_key(&d.sprite.0))
                .map_or(1.0, |d| d.scale());
            let plan = match self.gpu_on {
                true => gpu_water(&ctx, &mesh, matrix, params, scale),
                false => Err(GpuReject::Water),
            };
            let plan = plan.and_then(|u| {
                let ptr = *self
                    .gpu_water_meshes
                    .entry(model.0)
                    .or_insert_with(|| upload_water_mesh(&mesh));
                if ptr.is_null() {
                    Err(GpuReject::NoGpuMemory)
                } else {
                    Ok(GpuWaterDraw {
                        mesh: ptr,
                        uniforms: water_uniforms(&u),
                        texture: bind,
                        cull: u.cull_back as u32,
                    })
                }
            });
            let before = self.pending_water.len();
            let pending = &mut self.pending_water;
            let tnl = match plan {
                Ok(d) => {
                    let tris = (mesh.index_count / 3) as u32;
                    self.pending_gpu_water.push(d);
                    self.frame_stats.tris += tris;
                    let rings = trino_core::render3d::tessellate_ripples(
                        &ctx,
                        &mesh,
                        matrix,
                        params,
                        &mut |tri| pending.push(tri),
                    );
                    let mut s = TnlStats {
                        gpu_draws: 1,
                        gpu_tris: tris,
                        ..TnlStats::default()
                    };
                    s.add(&rings);
                    s
                }
                Err(r) => {
                    if self.gpu_on {
                        let n = &mut self.gpu_rejects[r.index()];
                        *n = n.saturating_add(1);
                    }
                    trino_core::render3d::tessellate_with(
                        &ctx,
                        &mut self.tnl_scratch,
                        &mesh,
                        matrix,
                        params,
                        &mut |tri| pending.push(tri),
                    )
                }
            };
            self.frame_stats.tnl.add(&tnl);
            let added = self.pending_water.len() - before;
            self.pending_water_tex
                .extend(core::iter::repeat_n(bind, added));
            return;
        }
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
        self.last_gpu_rejects = core::mem::take(&mut self.gpu_rejects);
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
