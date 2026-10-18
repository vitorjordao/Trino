//! RDP sprite renderer via the shim's `rdpq` wrappers, plus gouraud
//! triangles for the engine's software-T&L 3D (`trino_core::render3d`).

use alloc::boxed::Box;
use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::vec::Vec;
use core::ffi::c_void;
use core::sync::atomic::{AtomicU32, Ordering};

use trino_core::memo::{self, MemoKey, MemoSlot};
use trino_core::perf::RenderProfile;
use trino_core::render3d::{
    Camera3, DEFAULT_LIGHT, Light, MAX_JOINTS, Mesh, MeshLayout, NEAR_PLANE, RspPan, RspPart,
    ScreenTri, TexBind, TnlContext, TnlScratch, TnlStats, face_normal, flat_normal, flat_ratios,
    painter_index, painter_key, rsp_band, rsp_color, rsp_color_terms, rsp_eligible_mv,
    rsp_hull_inside, rsp_mesh_supported, rsp_part_bounds, rsp_rigid_check, rsp_rigid_draws,
    rsp_water_mesh_check, water_rsp, water_vertex,
};
use trino_core::text::{BitmapFont, GlyphIndex};
use trino_core::{
    BillboardParams, Caps, Color, FontId, FrameStats, LightList, LocalLight, Mat34, Material,
    ModelId, ModelParams, Renderer, Ripple, Shading, SpriteId, SpriteParams, TexFilter, TextParams,
    Transform3, Vec2, Vec3, WaterState,
};

use crate::ffi;
use crate::rsp::{self, RspMesh};
use crate::runtime::now_us;

/// Internal resolution the T&L projects to.
const SCREEN: Vec2 = Vec2::new(320.0, 240.0);

struct SpriteEntry {
    ptr: *mut c_void,
    /// Width and height in texels (an RSP textured draw scales its UVs).
    size: [f32; 2],
}

pub struct N64Renderer {
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
    /// What each pending triangle samples (sprite, clamp = billboard
    /// addressing, filter, detail layer).
    pending_tex: Vec<Option<TexBind>>,
    /// What `TexFilter::Default` means (`set_texture_filter`): the RDP's
    /// 3-point bilinear unless the game says otherwise.
    tex_filter: TexFilter,
    /// Draw order of the pending batch: (texture group, painter key).
    order: Vec<(u64, u64)>,
    camera: Camera3,
    light: Light,
    /// Local lights of this frame (per vertex, `Caps::N64` cap). A draw
    /// one reaches runs the CPU T&L (`RspReject::LocalLight`).
    locals: LightList,
    /// T&L constants of `camera` + `light` + `locals`, rebuilt when one
    /// changes.
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
    /// A colored command (`rsp::COLORED` in word 0) is followed by one more
    /// entry carrying its `rsp::color_words` (no field of its own: a new
    /// field in this struct moved hot data and cost every RSP draw, measured).
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
    /// This frame's water clock and ripples (`set_water`, cleared at
    /// `begin_frame`).
    water_state: WaterState,
    /// The batch's water triangles and what each samples (with its detail
    /// layer's scroll): drawn after the batch's CPU triangles and RSP
    /// commands, far to near, blending over them without writing depth.
    pending_water: Vec<ScreenTri>,
    pending_water_tex: Vec<Option<TexBind>>,
    /// Last frames' RSP commands by draw (`trino_core::memo`): a draw that
    /// repeats under an unchanged camera and light skips the eligibility
    /// test and the command build (~100 µs each on the console).
    /// Each entry is the command plus its [`RspPan`] (`MEMO_WORDS`): a
    /// camera that only moves (pan, zoom along the view axis) patches the
    /// command's translation instead of rebuilding it.
    memo: Vec<MemoSlot<MEMO_WORDS>>,
    /// Bumped whenever the view's rotation or focal, the light, the meshes
    /// or the cull mode change: older memo entries then never match.
    memo_gen: u32,
    /// Flat RSP copies (`rsp::build_flat`) by mesh and scale ratios
    /// (`flat_ratios`), baked the first time a mesh is drawn `Shading::Flat`
    /// on the RSP under them (`None`: it can't have one). Last:
    /// the hot fields above keep their offsets.
    rsp_flat: FlatCopies,
    /// Rigid skinned meshes' RSP parts (`rsp::build_part`), one per joint, and
    /// their bind-pose spheres (`rsp_part_bounds`): a posed draw becomes a
    /// command per joint on screen (`rsp_rigid_draws`).
    rsp_rigid: BTreeMap<u32, Rc<RigidCopy>>,
    /// Water surfaces' RSP copies (`rsp_trino_water`; `None`: the mesh can't have
    /// one), baked on their first water draw.
    rsp_water: BTreeMap<u32, Option<RspMesh>>,
    /// The batch's RSP water commands and their entries (`rsp::water_block`
    /// and ripple deltas live in `water_arena`), drawn first in the water
    /// bucket.
    pending_water_rsp: Vec<[[u32; rsp::DRAW_WORDS]; 2]>,
    water_arena: WaterArena,
}

/// Memo slots (268 bytes each): a few hundred draws per frame rarely share one.
const MEMO_SLOTS: usize = 1024;
/// A memo entry: the command, then `RspPan` (s, q, r2) and the view
/// translation the command was made for, as float bits, then a colored
/// command's color words (`rsp::color_words`).
const MEMO_WORDS: usize = rsp::DRAW_WORDS + 10 + rsp::COLOR_WORDS;
/// Where a colored command's color words sit in a memo entry.
const MEMO_COLOR: usize = rsp::DRAW_WORDS + 10;
/// Where the view translation sits in a memo entry.
const MEMO_T: usize = rsp::DRAW_WORDS + 7;

impl N64Renderer {
    /// Start timing every stage and model (see `trino_core::perf`).
    pub fn start_profile(&mut self) {
        self.profile = Some(RenderProfile::default());
    }

    /// Stop profiling and hand back what was measured.
    pub fn take_profile(&mut self) -> Option<RenderProfile> {
        self.profile.take()
    }

    fn rebuild_tnl(&mut self) {
        self.tnl_ctx = TnlContext::new(&self.camera, &self.light, SCREEN)
            .with_local_lights(self.locals.as_slice());
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
            tex_filter: TexFilter::Bilinear,
            order: Vec::new(),
            camera: Camera3::default(),
            tnl_ctx: TnlContext::new(&Camera3::default(), &DEFAULT_LIGHT, SCREEN),
            tnl_scratch: Box::new(TnlScratch::new()),
            light: DEFAULT_LIGHT,
            locals: LightList::EMPTY,
            caps: Caps::N64,
            profile: None,
            rsp_meshes: BTreeMap::new(),
            rsp_on: true,
            pending_rsp: Vec::new(),
            rsp_cull: 0,
            rsp_stream: false,
            memo: alloc::vec![MemoSlot::EMPTY; MEMO_SLOTS],
            memo_gen: 0,
            rsp_flat: BTreeMap::new(),
            rsp_rigid: BTreeMap::new(),
            rsp_water: BTreeMap::new(),
            pending_water_rsp: Vec::new(),
            water_arena: WaterArena::new(),
            water_state: WaterState::NONE,
            pending_water: Vec::new(),
            pending_water_tex: Vec::new(),
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
            self.submit_rsp();
            self.charge(t0, |s| &mut s.submit);
        }
        self.flush_water();
    }

    /// The batch's water, last: far to near, each surface's texture state
    /// bound once (`trino_3d_water`: blended, z-tested, not written).
    fn flush_water(&mut self) {
        if !self.pending_water_rsp.is_empty() {
            let t0 = self.tick();
            for [w, e] in &self.pending_water_rsp {
                submit_water(&self.sprites, w, e);
            }
            self.pending_water_rsp.clear();
            self.charge(t0, |s| &mut s.submit);
        }
        if self.pending_water.is_empty() {
            return;
        }
        self.frame_stats.tris += self.pending_water.len() as u32;
        let t0 = self.tick();
        self.order.clear();
        for (i, tri) in self.pending_water.iter().enumerate() {
            let group = self.pending_water_tex[i].map_or(0, |t| t.group());
            self.order.push((group, painter_key(tri.depth, i)));
        }
        self.order.sort_unstable();
        self.charge(t0, |s| &mut s.sort);
        let t1 = self.tick();
        let mut bound: Option<Option<TexBind>> = None;
        for &(_, key) in &self.order {
            let i = painter_index(key);
            let tex = self.pending_water_tex[i];
            let sprite = tex.and_then(|t| self.sprites.get(&t.id).map(|e| (e.ptr, t)));
            if bound != Some(tex) {
                let (ptr, dptr, flags, scroll) = match sprite {
                    Some((ptr, t)) => {
                        let detail = t
                            .detail
                            .and_then(|d| self.sprites.get(&d.sprite.0).map(|e| (e.ptr, d)));
                        let flags = ((t.filter == TexFilter::Point) as u32) << 1
                            | detail.map_or(0, |(_, d)| (d.scale_log2 as u32) << 4);
                        let [u, v] = t.water.unwrap_or([0, 0]);
                        (
                            ptr,
                            detail.map_or(core::ptr::null_mut(), |(p, _)| p),
                            flags,
                            (u as u32) << 16 | v as u32,
                        )
                    }
                    None => (core::ptr::null_mut(), core::ptr::null_mut(), 0, 0),
                };
                unsafe { ffi::trino_3d_water(ptr, dptr, flags, scroll) };
                bound = Some(tex);
            }
            if sprite.is_some() {
                emit_tri_tex(&self.pending_water[i]);
            } else {
                emit_tri(&self.pending_water[i]);
            }
        }
        self.charge(t1, |s| &mut s.submit);
        self.pending_water.clear();
        self.pending_water_tex.clear();
    }

    /// A water draw (`ModelParams::water`): the CPU T&L displaces and
    /// shades it with the frame's ripples, and adds their rings; it goes in
    /// the water bucket.
    fn draw_water(&mut self, model: ModelId, matrix: &Mat34, params: &ModelParams) {
        if self.rsp_on && self.draw_water_rsp(model, matrix, params) {
            return;
        }
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return;
        };
        let Some(w) = params.water else {
            return;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        self.frame_stats.models += 1;
        let texture = mesh
            .texture_for(params)
            .filter(|t| self.sprites.contains_key(&t.0));
        let t0 = self.tick();
        let ctx = self.tnl_ctx.with_water(&self.water_state);
        let before = self.pending_water.len();
        let pending = &mut self.pending_water;
        let tnl = trino_core::render3d::tessellate_with(
            &ctx,
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
        let time = self.water_state.time;
        let bind =
            texture.map(|t| TexBind::model(t.0, params, self.tex_filter).with_water(&w, time));
        let added = self.pending_water.len() - before;
        self.pending_water_tex
            .extend(core::iter::repeat_n(bind, added));
    }

    /// Hand every pending RSP command to the overlay, in order.
    fn submit_rsp(&mut self) {
        let mut i = 0;
        while i < self.pending_rsp.len() {
            let w = &self.pending_rsp[i];
            if w[0] & rsp::TEXTURED != 0 {
                submit_textured(&self.sprites, w, &self.pending_rsp[i + 1]);
                i += 2;
            } else if w[0] & rsp::COLORED != 0 {
                submit_colored(w, &self.pending_rsp[i + 1]);
                i += 2;
            } else {
                unsafe { ffi::trino_rsp_draw(w.as_ptr()) };
                i += 1;
            }
        }
        self.pending_rsp.clear();
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
                Some(t) => t.group(),
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
            let mut bound: Option<Option<TexBind>> = None;
            for &(_, key) in &self.order {
                let i = painter_index(key);
                let tex = self.pending_tex.get(i).copied().flatten();
                if bound != Some(tex) {
                    match tex.and_then(|t| self.sprites.get(&t.id).map(|e| (e, t))) {
                        Some((entry, t)) => {
                            let detail = t
                                .detail
                                .and_then(|d| self.sprites.get(&d.sprite.0).map(|e| (e.ptr, d)));
                            let flags = t.clamp as u32
                                | ((t.filter == TexFilter::Point) as u32) << 1
                                | detail.map_or(0, |(_, d)| (d.scale_log2 as u32) << 4);
                            let dptr = detail.map_or(core::ptr::null_mut(), |(p, _)| p);
                            unsafe { ffi::trino_3d_texture_ex(entry.ptr, dptr, flags) };
                        }
                        None => unsafe { ffi::trino_3d_begin() },
                    }
                    bound = Some(tex);
                }
                if tex.is_some_and(|t| self.sprites.contains_key(&t.id)) {
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
        let wh = unsafe { ffi::trino_sprite_size(ptr) };
        let size = [(wh >> 16) as f32, (wh & 0xFFFF) as f32];
        self.sprites.insert(id, SpriteEntry { ptr, size });
    }

    /// Record the texture of the `added` triangles appended at `before`
    /// (the tag list is only materialized once a textured draw joins).
    fn tag_pending(&mut self, before: usize, added: usize, key: Option<TexBind>) {
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
        self.rsp_flat.retain(|k, _| k.0 != id);
        self.rsp_rigid.remove(&id);
        self.rsp_water.remove(&id);
        // A translucent textured mesh is likely a water surface: its water copy
        // now, not on the first frame that draws it.
        if !mesh.opaque_colors() && rsp_water_mesh_check(&mesh).is_ok() {
            self.rsp_water.insert(id, rsp::build(&mesh));
        }
        if mesh.joint_count() > 0 && rsp_rigid_check(&mesh).is_ok() {
            self.rsp_rigid.insert(id, Rc::new(rigid_copy(&mesh)));
        }
        self.meshes.insert(id, (tmdl, layout));
        self.memo_gen = self.memo_gen.wrapping_add(1);
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
        self.memo_gen = self.memo_gen.wrapping_add(1);
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
        let mv = self.tnl_ctx.projection().0.mul(matrix);
        let Some(d) = rsp_eligible_mv(&self.tnl_ctx, &mesh, matrix, &mv, params) else {
            return Err("draw not RSP-eligible".into());
        };
        // A flat draw: its flat copy, each vertex lit by its face's normal.
        let flat = (params.shading == Shading::Flat).then(|| flat_ratios(matrix));
        let rm = match flat {
            Some(ratios) => {
                let key = (model.0, WHOLE, ratios);
                bake_flat(&mut self.rsp_flat, key, &mesh);
                self.rsp_flat.get(&key).and_then(Option::as_ref)
            }
            None => self.rsp_meshes.get(&model.0),
        };
        let Some(rm) = rm else {
            return Err("mesh has no RSP copy".into());
        };
        let seg0 = rm.seg0.clone();
        let n = seg0.len();
        let buf = unsafe { ffi::trino_alloc_uncached((n * 24) as u32) } as *mut u8;
        let phys = unsafe { ffi::trino_phys_addr(buf as *const c_void) };
        let cull = if d.cull_back { self.rsp_cull } else { 2 };
        let words = rsp::draw_words(&self.tnl_ctx, &mv, &d, params.tint, rm, cull, phys);
        let textured = rm.textured;
        let tex = mesh
            .texture_for(params)
            .and_then(|t| self.sprites.get(&t.0).map(|e| (t.0, e.ptr, e.size)));
        if textured && tex.is_none() {
            return Err("texture not loaded".into());
        }
        self.begin_frame(Color::BLACK);
        // The terms always (a white draw's equal its single intensity): no
        // float in an `Option` payload, whose garbage the FPU traps on once
        // LLVM hoists the math out of the branch (see `RspFog`).
        let terms = rsp_color_terms(&self.tnl_ctx, params);
        let colored = textured || rsp_color(&self.tnl_ctx, params).is_some();
        match tex {
            Some((id, ptr, size)) if textured => unsafe {
                let e = rsp::tex_entry(&terms, &mv, size, [0.0, 0.0], [id, 0, 0]);
                ffi::trino_3d_texture_ex(ptr, core::ptr::null_mut(), 0);
                ffi::trino_rsp_draw_textured(words.as_ptr(), e.as_ptr());
            },
            _ if colored => unsafe {
                ffi::trino_rsp_draw_colored(words.as_ptr(), rsp::color_words(&terms, &mv).as_ptr())
            },
            _ => unsafe { ffi::trino_rsp_draw(words.as_ptr()) },
        }
        self.end_frame();
        unsafe { ffi::trino_drain() };
        let (view, focal, screen) = self.tnl_ctx.projection();
        let mv = view.mul(matrix);
        let (cx, cy) = (screen.x * 0.5, screen.y * 0.5);
        let (mut dxy, mut dz, mut dc) = (0i32, 0i32, 0i32);
        // Textured: S, T (1/32 texel) and W, 1/W (relative error, 1/1000).
        let (mut dst, mut dw) = (0i32, 0i32);
        let tex_size = tex.map_or([1.0, 1.0], |t| t.2);
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
            let n = match flat {
                Some(ratios) => {
                    let t = core::array::from_fn(|j| mesh.index(3 * (k / 3) + j) as usize);
                    flat_normal(face_normal(&mesh, t), ratios)
                }
                None => mesh.normal(g),
            };
            let lit = n.dot(d.light);
            let lit = if lit > 0.0 { lit } else { 0.0 };
            let (amb, dif, e) = (terms.ambient, terms.diffuse, terms.emissive);
            let c = mesh.color(g);
            let t = params.tint;
            let ch = |v: u8, t: u8, i: usize| {
                let x = v as f32 * (t as f32 / 255.0) * (amb[i] + dif[i] * lit) + e[i] as f32;
                if x > 255.0 { 255.0 } else { x }
            };
            let mut want = [
                ch(c.r, t.r, 0),
                ch(c.g, t.g, 1),
                ch(c.b, t.b, 2),
                c.a as f32 * (t.a as f32 / 255.0),
            ];
            // Fog by view depth after the emissive add (alpha kept).
            let f = terms.fog;
            if f.on {
                let k = ((p.z - f.near) / (f.far - f.near)).clamp(0.0, 1.0);
                for i in 0..3 {
                    want[i] += (f.color[i] as f32 - want[i]) * k;
                }
            }
            for (ch, w) in want.iter().enumerate() {
                dc = dc.max((rd(8 + ch) as i32 - *w as i32).abs());
            }
            // No float from an `Option` payload: LLVM hoists float math out
            // of the branch and the FPU traps on a None's garbage (RspFog).
            if textured {
                let size = tex_size;
                let uv = mesh.uv(g);
                let es = (uv.x * size[0] * 32.0) as i32;
                let et = (uv.y * size[1] * 32.0) as i32;
                dst = dst.max((rd16(12) - es).abs()).max((rd16(14) - et).abs());
                let fx = |o: usize| {
                    let v = u32::from_be_bytes([rd(o), rd(o + 1), rd(o + 2), rd(o + 3)]);
                    v as i32 as f32 / 65536.0
                };
                let rel = |got: f32, want: f32| ((got / want - 1.0).abs() * 1000.0) as i32;
                dw = dw.max(rel(fx(16), p.z)).max(rel(fx(20), 1.0 / p.z));
            }
        }
        let mut msg = alloc::format!("{n} vertices: xy {dxy}/4 px, z {dz}/32767, color {dc}/255");
        if textured {
            msg.push_str(&alloc::format!(", st {dst}/32 texel, w {dw}/1000"));
        }
        if dxy > 2 || dz > 64 || dc > 3 || dst > 2 || dw > 12 {
            Err(msg)
        } else {
            Ok(msg)
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

impl Default for N64Renderer {
    fn default() -> Self {
        Self::new()
    }
}

/// One colored RSP command: its 27 words and the entry carrying its color
/// words. Out of line and cold: white-light frames never run it.
#[cold]
#[inline(never)]
fn submit_colored(w: &[u32; rsp::DRAW_WORDS], carrier: &[u32; rsp::DRAW_WORDS]) {
    unsafe { ffi::trino_rsp_draw_colored(w.as_ptr(), carrier.as_ptr()) };
}

/// One textured RSP command: its texture and mode bound by the binding in
/// its carrier entry (`rsp::TEX_BIND`, as `flush_cpu_batch` binds a CPU
/// triangle group), then the command with its color and texture words.
#[cold]
#[inline(never)]
fn submit_textured(
    sprites: &BTreeMap<u32, SpriteEntry>,
    w: &[u32; rsp::DRAW_WORDS],
    carrier: &[u32; rsp::DRAW_WORDS],
) {
    let bind = &carrier[rsp::TEX_BIND..rsp::TEX_BIND + 3];
    let Some(base) = sprites.get(&bind[0]) else {
        return;
    };
    let detail = sprites
        .get(&bind[1])
        .map_or(core::ptr::null_mut(), |e| e.ptr);
    unsafe {
        ffi::trino_3d_texture_ex(base.ptr, detail, bind[2]);
        ffi::trino_rsp_draw_textured(w.as_ptr(), carrier.as_ptr());
    }
}

/// Microseconds the profiled frames waited for the RSP and RDP at
/// `end_frame` (`trino_drain`), since the last [`take_drain_us`]. A static,
/// not a renderer or profile field: growing those structs moved hot data
/// and slowed every RSP draw (measured in ares, 2026-10-16).
static DRAIN_US: AtomicU32 = AtomicU32::new(0);

/// [`DRAIN_US`], reset.
pub fn take_drain_us() -> u32 {
    let d = DRAIN_US.load(Ordering::Relaxed);
    DRAIN_US.store(0, Ordering::Relaxed);
    d
}

/// Gouraud triangle (vertex colors only).
/// A memoized draw whose sphere left the band: are its vertices still
/// inside ([`rsp_hull_inside`])? Out of line: it's off the hot path.
#[inline(never)]
fn hull_inside(
    ctx: &TnlContext,
    meshes: &BTreeMap<u32, (Vec<u8>, MeshLayout)>,
    model: ModelId,
    matrix: &Mat34,
) -> bool {
    meshes.get(&model.0).is_some_and(|(tmdl, layout)| {
        let mesh = Mesh::from_layout(tmdl, layout);
        rsp_hull_inside(ctx, &mesh, &ctx.projection().0.mul(matrix))
    })
}

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

impl N64Renderer {
    /// Queue one RSP draw command. Streaming: hand the overlay a chunk as
    /// soon as it is full (and the batch holds no CPU triangles), so the
    /// RSP starts while the CPU is still issuing draws. Single-draw
    /// streaming hung the RDP in ares under heavy 2D traffic (see the
    /// n64-performance skill); chunks keep the proven burst pattern.
    fn queue_rsp(&mut self, words: [u32; rsp::DRAW_WORDS]) {
        self.pending_rsp.push(words);
        if self.rsp_stream
            && self.pending_tris.is_empty()
            && self.pending_rsp.len() >= RSP_STREAM_CHUNK
        {
            self.submit_rsp();
        }
    }

    /// Statistics and profile of one RSP draw of `tris` triangles.
    fn count_rsp(&mut self, model: ModelId, t0: u32, tris: u32) {
        let tnl = TnlStats {
            rsp_draws: 1,
            rsp_tris: tris,
            ..TnlStats::default()
        };
        self.frame_stats.tnl.add(&tnl);
        if let Some(p) = &mut self.profile {
            p.record_model(model, now_us().wrapping_sub(t0), &tnl);
        }
    }
}

/// Glyphs per `trino_text_run` call (a stack buffer of 3 words each).
const TEXT_RUN: usize = 32;

/// A registered font: its glyph table and the direct index of its
/// codepoints below 256 (built once, at register).
struct FontEntry {
    table: alloc::boxed::Box<[u8]>,
    index: GlyphIndex,
}

impl FontEntry {
    fn font(&self) -> Option<BitmapFont<'_>> {
        BitmapFont::parse(&self.table)
            .ok()
            .map(|f| f.with_index(&self.index))
    }
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
        self.water_state = WaterState::NONE;
        self.water_arena.next_frame();
        if self.locals.set(&[], 0) {
            self.rebuild_tnl();
        }
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
        // The same camera every frame (most frames of most games) keeps
        // the T&L constants and the memo.
        if *camera != self.camera {
            let before = self.tnl_ctx.projection();
            self.camera = *camera;
            self.rebuild_tnl();
            // Only the view's translation moved (a pan): remembered
            // commands are patched, not dropped.
            let after = self.tnl_ctx.projection();
            let turned = (0..3).any(|r| (0..3).any(|c| before.0.m[r][c] != after.0.m[r][c]))
                || before.1 != after.1;
            if turned {
                self.memo_gen = self.memo_gen.wrapping_add(1);
            }
        }
    }

    fn set_light(&mut self, light: &Light) {
        // Lighting is resolved into vertex colors at tessellation time, so
        // a change needs no batch flush.
        if *light != self.light {
            self.light = *light;
            self.rebuild_tnl();
            self.memo_gen = self.memo_gen.wrapping_add(1);
        }
    }

    fn set_texture_filter(&mut self, filter: TexFilter) {
        self.tex_filter = match filter {
            TexFilter::Default => TexFilter::Bilinear,
            f => f,
        };
    }

    fn set_local_lights(&mut self, lights: &[LocalLight]) {
        // Per vertex, no flush. The memo needs no new generation: a
        // remembered command is an RSP draw no light reached, and a hit
        // tests the lights again (`draw_model_matrix`).
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
        if params.water.is_some() {
            self.draw_water(model, matrix, params);
            return;
        }
        // A rigid skinned mesh: a command per joint on screen, when every
        // part qualifies.
        if self.rsp_on
            && let Some(rc) = self.rsp_rigid.get(&model.0).cloned()
            && self.draw_rigid(model, matrix, params, &rc)
        {
            return;
        }
        // A draw that repeats under the same camera and light: last
        // frame's command.
        let key = if self.rsp_on {
            MemoKey::new(model.0, self.memo_gen, matrix, params)
        } else {
            None
        };
        if let Some(k) = &key
            && let Some(hit) = memo::find_mut(&mut self.memo, k)
        {
            let v = &self.tnl_ctx.projection().0.m;
            let t = [v[0][3].to_bits(), v[1][3].to_bits(), v[2][3].to_bits()];
            let w = &mut hit.words;
            let still = w[MEMO_T..MEMO_T + 3] == t;
            let f = |w: &[u32; MEMO_WORDS], i: usize| f32::from_bits(w[rsp::DRAW_WORDS + i]);
            let pan = RspPan {
                s: [f(w, 0), f(w, 1), f(w, 2)],
                q: [f(w, 3), f(w, 4), f(w, 5)],
                r2: f(w, 6),
            };
            let mt = pan.translation([v[0][3], v[1][3], v[2][3]]);
            // A local light reaching it now: the CPU T&L lights it.
            let lit = !self.locals.is_empty()
                && params.shading != Shading::Unlit
                && self.tnl_ctx.lights_touch(pan.center(mt), pan.r2);
            // Moved: still inside the band where the camera is now (its
            // sphere, or else its vertices: a ground tile whose sphere
            // crosses the band)? Else the draw takes the full path (maybe
            // the CPU's).
            // A colored command (fog, a colored light) is reused only under
            // the same camera: its fog row follows the view, and a pan may
            // move it into the fog band where its edges need splits.
            let colored = w[0] & rsp::COLORED != 0;
            if !lit
                && (still
                    || (!colored
                        && (rsp_band(&self.tnl_ctx, pan.center(mt), pan.r2)
                            || hull_inside(&self.tnl_ctx, &self.meshes, model, matrix))))
            {
                let t0 = if self.profile.is_some() { now_us() } else { 0 };
                if !still {
                    let mut c = [0u32; rsp::DRAW_WORDS];
                    c.copy_from_slice(&w[..rsp::DRAW_WORDS]);
                    rsp::patch_translation(&self.tnl_ctx, &mut c, mt);
                    w[..rsp::DRAW_WORDS].copy_from_slice(&c);
                    w[MEMO_T..MEMO_T + 3].copy_from_slice(&t);
                }
                let mut words = [0u32; rsp::DRAW_WORDS];
                words.copy_from_slice(&w[..rsp::DRAW_WORDS]);
                let tris = hit.extra;
                self.frame_stats.models += 1;
                if colored {
                    let mut c = [0u32; rsp::DRAW_WORDS];
                    c[..rsp::COLOR_WORDS].copy_from_slice(&w[MEMO_COLOR..]);
                    self.pending_rsp.push(words);
                    self.queue_rsp(c);
                } else {
                    self.queue_rsp(words);
                }
                self.count_rsp(model, t0, tris);
                return;
            }
        }
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
        // A flat draw goes out with the mesh's flat copy (face normals).
        let rsp = match rsp {
            Some((_, d, mv)) if params.shading == Shading::Flat => {
                let key = (model.0, WHOLE, flat_ratios(matrix));
                bake_flat(&mut self.rsp_flat, key, &mesh);
                let rm = self.rsp_flat.get(&key).and_then(Option::as_ref);
                rm.map(|rm| (rm, d, mv))
            }
            r => r,
        };
        // A textured copy needs its texture loaded: without it the CPU path
        // draws the mesh untextured.
        let tex = match &rsp {
            Some((rm, ..)) if rm.textured => mesh
                .texture_for(params)
                .and_then(|t| self.sprites.get(&t.0).map(|e| (t.0, e.size))),
            _ => None,
        };
        let rsp = rsp.filter(|(rm, ..)| !rm.textured || tex.is_some());
        if let Some((rm, d, mv)) = rsp {
            // The overlay transforms, lights and rasterizes the whole draw;
            // the command goes out with the batch (flush_model_batch).
            let t0 = self.tick();
            let cull = if d.cull_back { self.rsp_cull } else { 2 };
            let words = rsp::draw_words(&self.tnl_ctx, &mv, &d, params.tint, rm, cull, 0);
            let tris = (mesh.index_count / 3) as u32;
            // Textured (44 words, never remembered): the texture's binding,
            // color and texture words ride in the next entry.
            if let Some((id, size)) = tex {
                let b = TexBind::model(id, params, self.tex_filter);
                let detail = b.detail.filter(|d| self.sprites.contains_key(&d.sprite.0));
                let flags = b.clamp as u32
                    | ((b.filter == TexFilter::Point) as u32) << 1
                    | detail.map_or(0, |d| (d.scale_log2 as u32) << 4);
                let bind = [id, detail.map_or(0, |d| d.sprite.0), flags];
                let c = rsp_color_terms(&self.tnl_ctx, params);
                let entry = rsp::tex_entry(&c, &mv, size, [0.0, 0.0], bind);
                let mut words = words;
                words[0] |= rsp::TEXTURED;
                self.pending_rsp.push(words);
                self.queue_rsp(entry);
                self.count_rsp(model, t0, tris);
                return;
            }
            // The colored command (40 words): its color words ride in the
            // next queue entry, queued together, never split by a streamed
            // chunk.
            let color = rsp_color(&self.tnl_ctx, params).map(|c| rsp::color_entry(&c, &mv));
            let mut words = words;
            if color.is_some() {
                words[0] |= rsp::COLORED;
            }
            if let Some(k) = key
                && let Some(pan) = RspPan::new(&self.tnl_ctx.projection().0, &mv, &mesh, matrix)
            {
                let mut entry = [0u32; MEMO_WORDS];
                entry[..rsp::DRAW_WORDS].copy_from_slice(&words);
                if let Some(c) = &color {
                    entry[MEMO_COLOR..].copy_from_slice(&c[..rsp::COLOR_WORDS]);
                }
                let v = &self.tnl_ctx.projection().0.m;
                let aux = [
                    pan.s[0], pan.s[1], pan.s[2], pan.q[0], pan.q[1], pan.q[2], pan.r2, v[0][3],
                    v[1][3], v[2][3],
                ];
                for (i, a) in aux.iter().enumerate() {
                    entry[rsp::DRAW_WORDS + i] = a.to_bits();
                }
                memo::store(&mut self.memo, k, entry, tris);
            }
            match color {
                Some(c) => {
                    self.pending_rsp.push(words);
                    self.queue_rsp(c);
                }
                None => self.queue_rsp(words),
            }
            self.count_rsp(model, t0, tris);
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
        let t0 = self.tick();
        unsafe { ffi::trino_frame_end() }
        // Profiling: charge the RSP/RDP work this frame queued to the frame
        // itself (with RSP T&L it is no longer CPU time).
        if self.profile.is_some() {
            let t1 = now_us();
            unsafe { ffi::trino_drain() }
            // Plain load and store (one thread; no read-modify-write
            // atomics needed on the VR4300).
            let d = now_us().wrapping_sub(t1);
            DRAIN_US.store(
                DRAIN_US.load(Ordering::Relaxed).wrapping_add(d),
                Ordering::Relaxed,
            );
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
        let Some(entry) = self.fonts.get(&font.0).cloned() else {
            return;
        };
        let Some(parsed) = entry.font() else {
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
                // Glyphs go to the shim in runs (one call per run, not per
                // glyph).
                let mut run = [0u32; 3 * TEXT_RUN];
                let mut n = 0usize;
                let mut glyphs = 0u32;
                trino_core::text::layout(&parsed, pos, text, params, &mut |r, at| {
                    let xy = ((at.x as i32 as u32 & 0xFFFF) << 16) | (at.y as i32 as u32 & 0xFFFF);
                    let st = ((r.pos.x as u32) << 16) | r.pos.y as u32;
                    let wh = ((r.size.x as u32) << 16) | r.size.y as u32;
                    run[3 * n..3 * n + 3].copy_from_slice(&[xy, st, wh]);
                    n += 1;
                    if n == TEXT_RUN {
                        unsafe { ffi::trino_text_run(run.as_ptr(), n as u32, k) };
                        n = 0;
                    }
                    glyphs += 1;
                });
                if n > 0 {
                    unsafe { ffi::trino_text_run(run.as_ptr(), n as u32, k) };
                }
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
            .and_then(|e| e.font())
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

/// Flat RSP copies by (mesh, part, scale ratios): see `N64Renderer::rsp_flat`.
/// The part is a rigid mesh's joint, or [`WHOLE`].
type FlatKey = (u32, u32, [u16; 3]);
type FlatCopies = BTreeMap<FlatKey, Option<RspMesh>>;
/// A flat copy of the whole mesh (not a rigid part).
const WHOLE: u32 = u32::MAX;

/// A rigid mesh's RSP parts by joint and their bind-pose spheres.
pub struct RigidCopy {
    parts: Vec<Option<RspMesh>>,
    bounds: [Option<(Vec3, f32)>; MAX_JOINTS],
}

/// Bake a flat RSP copy (`rsp::build_flat`) on its first draw under these
/// scale ratios.
fn bake_flat(copies: &mut FlatCopies, key: FlatKey, mesh: &Mesh) {
    if !copies.contains_key(&key) {
        bake_flat_cold(copies, key, mesh);
    }
}

/// Cold: once per mesh, and castle64 never draws flat.
#[cold]
#[inline(never)]
fn bake_flat_cold(copies: &mut FlatCopies, key: FlatKey, mesh: &Mesh) {
    let part = (key.1 != WHOLE).then_some(key.1 as usize);
    copies.insert(key, rsp::build_flat(mesh, part, key.2));
}

/// The RSP parts of rigid mesh `mesh` ([`RigidCopy`]).
#[cold]
#[inline(never)]
fn rigid_copy(mesh: &Mesh) -> RigidCopy {
    RigidCopy {
        parts: (0..mesh.joint_count())
            .map(|j| rsp::build_part(mesh, j))
            .collect(),
        bounds: core::array::from_fn(|j| {
            if j < mesh.joint_count() {
                rsp_part_bounds(mesh, j)
            } else {
                None
            }
        }),
    }
}

impl N64Renderer {
    /// A posed rigid draw on the RSP, a command per joint on screen
    /// (`rsp_rigid_draws`); `false` when a part needs the CPU path (the
    /// draw is then tessellated whole). Out of line: castle64's skinned
    /// characters are split into static parts already.
    #[inline(never)]
    fn draw_rigid(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        params: &ModelParams,
        rc: &RigidCopy,
    ) -> bool {
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return false;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        let t0 = self.tick();
        let mut joints = [Mat34::IDENTITY; MAX_JOINTS];
        mesh.pose(params.anim, &mut joints);
        let mut parts = [RspPart {
            joint: 0,
            model: Mat34::IDENTITY,
            mv: Mat34::IDENTITY,
            draw: trino_core::render3d::RspDraw {
                light: Vec3::ZERO,
                ambient: 1.0,
                cull_back: true,
            },
        }; MAX_JOINTS];
        let Ok(n) = rsp_rigid_draws(
            &self.tnl_ctx,
            &mesh,
            matrix,
            params,
            &joints,
            &rc.bounds,
            &mut parts,
        ) else {
            return false;
        };
        let color = rsp_color(&self.tnl_ctx, params);
        let mut out: Vec<([u32; rsp::DRAW_WORDS], Option<[u32; rsp::DRAW_WORDS]>, u32)> =
            Vec::with_capacity(n);
        for p in &parts[..n] {
            let rm = if params.shading == Shading::Flat {
                let key = (model.0, p.joint as u32, flat_ratios(&p.model));
                bake_flat(&mut self.rsp_flat, key, &mesh);
                self.rsp_flat.get(&key).and_then(Option::as_ref)
            } else {
                rc.parts.get(p.joint).and_then(Option::as_ref)
            };
            let Some(rm) = rm else {
                return false;
            };
            let cull = if p.draw.cull_back { self.rsp_cull } else { 2 };
            let mut words =
                rsp::draw_words(&self.tnl_ctx, &p.mv, &p.draw, params.tint, rm, cull, 0);
            let entry = color.map(|c| rsp::color_entry(&c, &p.mv));
            if entry.is_some() {
                words[0] |= rsp::COLORED;
            }
            out.push((words, entry, rm.tris()));
        }
        self.frame_stats.models += 1;
        for (k, (words, entry, tris)) in out.into_iter().enumerate() {
            // The whole build is charged to the first part.
            let t = if k == 0 { t0 } else { self.tick() };
            match entry {
                Some(c) => {
                    self.pending_rsp.push(words);
                    self.queue_rsp(c);
                }
                None => self.queue_rsp(words),
            }
            self.count_rsp(model, t, tris);
        }
        true
    }
}

/// Uncached RDRAM for the RSP water draws' per-frame data (water blocks and
/// ripple deltas): three buffers in turn, one per frame, so the CPU never
/// writes one the RSP may still be reading from the frame before.
struct WaterArena {
    bufs: [*mut u8; WATER_ARENA_FRAMES],
    phys: [u32; WATER_ARENA_FRAMES],
    cur: usize,
    used: usize,
}

const WATER_ARENA_FRAMES: usize = 3;
/// Bytes per frame: a dozen water draws of 64 vertices with room to spare.
const WATER_ARENA_BYTES: usize = 16 * 1024;

impl WaterArena {
    const fn new() -> Self {
        WaterArena {
            bufs: [core::ptr::null_mut(); WATER_ARENA_FRAMES],
            phys: [0; WATER_ARENA_FRAMES],
            cur: 0,
            used: 0,
        }
    }

    fn next_frame(&mut self) {
        self.cur = (self.cur + 1) % WATER_ARENA_FRAMES;
        self.used = 0;
        if self.bufs[self.cur].is_null() {
            self.grow();
        }
    }

    /// This frame's buffer, allocated once (cold: the first frames only).
    #[cold]
    #[inline(never)]
    fn grow(&mut self) {
        let p = unsafe { ffi::trino_alloc_uncached(WATER_ARENA_BYTES as u32) } as *mut u8;
        if !p.is_null() {
            self.bufs[self.cur] = p;
            self.phys[self.cur] = unsafe { ffi::trino_phys_addr(p as *const c_void) };
        }
    }

    /// `bytes` (8-aligned) of this frame's buffer: (pointer, RSP address).
    fn alloc(&mut self, bytes: usize) -> Option<(*mut u8, u32)> {
        let bytes = bytes.div_ceil(8) * 8;
        if self.bufs[self.cur].is_null() {
            return None;
        }
        if self.used + bytes > WATER_ARENA_BYTES {
            return None;
        }
        let at = self.used;
        self.used += bytes;
        Some((
            unsafe { self.bufs[self.cur].add(at) },
            self.phys[self.cur] + at as u32,
        ))
    }
}

/// Copy `bytes` (big-endian words) to uncached memory at `dst`.
fn write_uncached(dst: *mut u8, bytes: &[u8]) {
    for (k, w) in bytes.as_chunks::<4>().0.iter().enumerate() {
        unsafe {
            (dst as *mut u32)
                .add(k)
                .write_volatile(u32::from_be_bytes(*w))
        };
    }
}

/// One RSP water command: its texture and blend bound by the binding in its
/// entry (`rsp::WATER_BIND`, as `flush_water` binds a CPU group), then the
/// command.
#[cold]
#[inline(never)]
fn submit_water(
    sprites: &BTreeMap<u32, SpriteEntry>,
    w: &[u32; rsp::DRAW_WORDS],
    e: &[u32; rsp::DRAW_WORDS],
) {
    let bind = &e[rsp::WATER_BIND..rsp::WATER_BIND + 4];
    let Some(base) = sprites.get(&bind[0]) else {
        return;
    };
    let detail = sprites
        .get(&bind[1])
        .map_or(core::ptr::null_mut(), |s| s.ptr);
    unsafe {
        ffi::trino_3d_water(base.ptr, detail, bind[2], bind[3]);
        ffi::trino_rsp_draw_water(w.as_ptr(), e.as_ptr());
    }
}

impl N64Renderer {
    /// A water draw on the RSP (`rsp_trino_water`): the surface displaced,
    /// lit and shaded by the overlay, the ripples' deltas computed here for
    /// its vertices, the ring decals still from the CPU T&L. `false` when
    /// the draw needs the CPU path (`rsp_check`'s water rule).
    #[inline(never)]
    fn draw_water_rsp(&mut self, model: ModelId, matrix: &Mat34, params: &ModelParams) -> bool {
        let t0 = self.tick();
        let Some((words, e, tris, bind)) = self.water_command(model, matrix, params, 0) else {
            return false;
        };
        let ctx = self.tnl_ctx.with_water(&self.water_state);
        let Some((tmdl, layout)) = self.meshes.get(&model.0) else {
            return false;
        };
        let mesh = Mesh::from_layout(tmdl, layout);
        // The ring decals: CPU triangles in the water bucket, after.
        let before = self.pending_water.len();
        let pending = &mut self.pending_water;
        let rings =
            trino_core::render3d::tessellate_ripples(&ctx, &mesh, matrix, params, &mut |tri| {
                pending.push(tri)
            });
        let added = self.pending_water.len() - before;
        self.pending_water_tex
            .extend(core::iter::repeat_n(Some(bind), added));
        self.pending_water_rsp.push([words, e]);
        self.frame_stats.models += 1;
        self.frame_stats.tnl.add(&rings);
        self.count_rsp(model, t0, tris);
        true
    }

    /// A water draw's RSP command, its entry (color and texture words, the
    /// water block's and ripple deltas' addresses in `water_arena`, its
    /// binding), its triangles and its texture binding (for the rings).
    /// `debug_phys` as `rsp::draw_words`' (the oracle's read-back).
    fn water_command(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        params: &ModelParams,
        debug_phys: u32,
    ) -> Option<([u32; rsp::DRAW_WORDS], [u32; rsp::DRAW_WORDS], u32, TexBind)> {
        let w = params.water?;
        let ctx = self.tnl_ctx.with_water(&self.water_state);
        let (tmdl, layout) = self.meshes.get(&model.0)?;
        let mesh = Mesh::from_layout(tmdl, layout);
        if !self.rsp_water.contains_key(&model.0) {
            let copy = rsp_water_mesh_check(&mesh)
                .ok()
                .and_then(|_| rsp::build(&mesh));
            self.rsp_water.insert(model.0, copy);
        }
        let rm = self.rsp_water.get(&model.0)?.as_ref()?;
        let mv = ctx.projection().0.mul(matrix);
        let d = rsp_eligible_mv(&ctx, &mesh, matrix, &mv, params)?;
        let t = mesh.texture_for(params)?;
        let size = self.sprites.get(&t.0)?.size;
        let ws = water_rsp(&ctx, &mesh, matrix, params)?;
        let n = rm.order.len();
        let (ptr, phys) = self
            .water_arena
            .alloc(rsp::WATER_BLOCK + n * rsp::RIPPLE_DELTA)?;
        write_uncached(ptr, &rsp::water_block(&ctx, matrix, &ws));
        let mut deltas = alloc::vec![0u8; n * rsp::RIPPLE_DELTA];
        if ws.n_ripples > 0 {
            for (k, &g) in rm.order.iter().enumerate() {
                let p = matrix.transform_point(mesh.position(g as usize));
                let at = k * rsp::RIPPLE_DELTA;
                deltas[at..at + rsp::RIPPLE_DELTA]
                    .copy_from_slice(&rsp::ripple_delta(&ws, p.x, p.z));
            }
        }
        write_uncached(unsafe { ptr.add(rsp::WATER_BLOCK) }, &deltas);
        let cull = if d.cull_back { self.rsp_cull } else { 2 };
        let words = rsp::draw_words(&ctx, &mv, &d, params.tint, rm, cull, debug_phys);
        let c = rsp_color_terms(&ctx, params);
        let scroll = [ws.uv.x * size[0], ws.uv.y * size[1]];
        let mut e = rsp::tex_entry(&c, &mv, size, scroll, [0; 3]);
        e[rsp::WATER_PTRS] = phys;
        e[rsp::WATER_PTRS + 1] = phys + rsp::WATER_BLOCK as u32;
        let b = TexBind::model(t.0, params, self.tex_filter).with_water(&w, ctx.water().time);
        let detail = b.detail.filter(|d| self.sprites.contains_key(&d.sprite.0));
        let [u, v] = b.water.unwrap_or([0, 0]);
        e[rsp::WATER_BIND] = t.0;
        e[rsp::WATER_BIND + 1] = detail.map_or(0, |d| d.sprite.0);
        e[rsp::WATER_BIND + 2] = ((b.filter == TexFilter::Point) as u32) << 1
            | detail.map_or(0, |d| (d.scale_log2 as u32) << 4);
        e[rsp::WATER_BIND + 3] = (u as u32) << 16 | v as u32;
        Some((words, e, rm.tris(), b))
    }
}

impl N64Renderer {
    /// The water overlay against the CPU path (`water_vertex`): a water
    /// draw's first segment, screen position, depth, color and S, T per
    /// vertex, under the frame's camera and light and `time`, `ripples`.
    pub fn rsp_water_oracle(
        &mut self,
        model: ModelId,
        matrix: &Mat34,
        params: &ModelParams,
        time: f32,
        ripples: &[Ripple],
    ) -> Result<alloc::string::String, alloc::string::String> {
        self.begin_frame(Color::BLACK);
        self.water_state = WaterState::new(time, ripples, self.caps.max_ripples);
        let Some((tmdl, layout)) = self.meshes.get(&model.0).cloned() else {
            return Err("mesh not registered".into());
        };
        let mesh = Mesh::from_layout(&tmdl, &layout);
        let probe = unsafe { ffi::trino_alloc_uncached(32 * 24) } as *mut u8;
        let phys = unsafe { ffi::trino_phys_addr(probe as *const c_void) };
        let Some((words, e, _, _)) = self.water_command(model, matrix, params, phys) else {
            return Err("water draw not RSP-eligible".into());
        };
        submit_water(&self.sprites, &words, &e);
        self.end_frame();
        unsafe { ffi::trino_drain() };
        let Some(Some(rm)) = self.rsp_water.get(&model.0) else {
            return Err("no water copy".into());
        };
        let seg0 = rm.seg0.clone();
        let ctx = self.tnl_ctx.with_water(&self.water_state);
        let (_, focal, screen) = ctx.projection();
        let (cx, cy) = (screen.x * 0.5, screen.y * 0.5);
        let size = mesh
            .texture_for(params)
            .and_then(|t| self.sprites.get(&t.0))
            .map_or([1.0, 1.0], |s| s.size);
        let ws = water_rsp(&ctx, &mesh, matrix, params);
        let scroll = ws.map_or(Vec2::new(0.0, 0.0), |w| w.uv);
        let (mut dxy, mut dz, mut dc, mut dst) = (0i32, 0i32, 0i32, 0i32);
        let mut worst = alloc::string::String::new();
        for (k, &g) in seg0.iter().enumerate() {
            let g = g as usize;
            let rd = |o: usize| unsafe { probe.add(k * 24 + o).read_volatile() };
            let rd16 = |o: usize| i16::from_be_bytes([rd(o), rd(o + 1)]) as i32;
            let Some((p, c)) = water_vertex(&ctx, &mesh, matrix, params, g) else {
                return Err("not a water draw".into());
            };
            let ex = ((cx + p.x * focal / p.z) * 4.0) as i32;
            let ey = ((cy - p.y * focal / p.z) * 4.0) as i32;
            let ez = ((1.0 - NEAR_PLANE / p.z) * 32767.0) as i32;
            dxy = dxy.max((rd16(0) - ex).abs()).max((rd16(2) - ey).abs());
            dz = dz.max((rd16(4) - ez).abs());
            let got = [rd(8), rd(9), rd(10), rd(11)];
            let want = [c.r, c.g, c.b, c.a];
            let d = (0..4)
                .map(|ch| (got[ch] as i32 - want[ch] as i32).abs())
                .max()
                .unwrap_or(0);
            if d > dc {
                worst = alloc::format!(" (vertex {k}: got {got:?}, want {want:?})");
            }
            dc = dc.max(d);
            let uv = mesh.uv(g);
            let es = ((uv.x + scroll.x) * size[0] * 32.0) as i32;
            let et = ((uv.y + scroll.y) * size[1] * 32.0) as i32;
            dst = dst.max((rd16(12) - es).abs()).max((rd16(14) - et).abs());
        }
        let msg = alloc::format!(
            "{} vertices: xy {dxy}/4 px, z {dz}/32767, color {dc}/255, st {dst}/32 texel{worst}",
            seg0.len()
        );
        if dxy > 4 || dz > 64 || dc > 6 || dst > 4 {
            Err(msg)
        } else {
            Ok(msg)
        }
    }
}
