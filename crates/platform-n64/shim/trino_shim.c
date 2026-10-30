// Trino's C shim over libdragon.
//
// WHY A SHIM: libdragon is compiled with gcc -mabi=o64; Rust/LLVM has no o64
// so the Rust side codegens with the closest ABI (n32-flavored, 32-bit
// pointers). The two agree on simple calls but NOT on every corner
// (by-value structs, varargs, >4 args, long doubles). This file keeps the
// entire C<->Rust surface inside the safe subset:
//
//   - max 4 arguments, all of them i32/u32/f32 or pointers
//   - no by-value structs (pass pointers to #[repr(C)] structs instead)
//   - no variadics (format on the Rust side, pass the final string)
//   - returns are void, i32/u32, f32 or a pointer
//
// It is compiled INSIDE the Docker toolchain image (real libdragon headers,
// gcc -mabi=o64), so any API drift in libdragon is a compile error here —
// never silent UB. Rust declarations live in ../src/ffi.rs and MUST match.

#include <stdint.h>
#include <stdlib.h>
#include <malloc.h>
#include <string.h>
#include <libdragon.h>

// ---------------------------------------------------------------------------
// Boot + logging

static void trino_check_trifmt(void);

// Trino's RSP overlay (rsp_trino.S): vertex T&L for static meshes.
DEFINE_RSP_UCODE(rsp_trino);
static uint32_t trino_rsp_id;
// The colored overlay (defined last in this file, see there).
static void trino_register_color_overlay(void);
// The textured overlay (last in this file too).
static void trino_register_tex_overlay(void);
// The water overlay (last in this file too).
static void trino_register_water_overlay(void);

// VI interrupt counter: an independent clock for perf reports (60 per
// emulated second on NTSC), to cross-check the count-register timings.
static volatile uint32_t trino_vblanks = 0;
static void trino_on_vi(void) { trino_vblanks++; }

// RSP T&L bookkeeping. RSP draws need the 3D shade + z mode current when
// the overlay runs (they don't go through trino_3d_begin's batch flush), and
// rdpq's CPU-side autosync never sees the triangles the overlay emits, so
// the next mode change must be preceded by an explicit SYNC_PIPE.
static int trino_mode_3d = 0;
static int trino_rsp_pending = 0;
// Textured RSP triangles were queued since the last mode change: rdpq never
// saw them use TMEM and the tiles, so the next upload needs explicit syncs.
static int trino_rsp_tex_pending = 0;
// 2D state cache: standard mode + alpha compare already set for blits, and
// the tint the combiner currently applies (0xFFFFFFFF = none). Consecutive
// blits (every glyph of a string) then skip the mode setup — one SOM change
// and its syncs per blit were most of a glyph's cost. Anything else that
// touches the mode clears it (trino_mode_standard).
static int trino_2d_ready = 0;
static uint32_t trino_2d_tint = 0xFFFFFFFFu;
static int trino_fill_ready;
// The sprite whose texels TILE0/TMEM hold right now (text atlas), or NULL
// once anything else may have loaded TMEM (a blit, a textured 3D draw, a
// new frame): consecutive strings then skip the atlas upload.
static void* trino_tmem_owner = NULL;
// The textured 3D state trino_3d_texture_ex left: the sprites TILE0/TILE1
// hold and their flags (trino_t3_sp NULL once anything else loaded TMEM),
// and whether the render mode is still its own (cleared by every mode
// change). A run of draws with one texture (the showcase's terrain tiles)
// then uploads it once instead of once per draw.
static void* trino_t3_sp = NULL;
static void* trino_t3_dt = NULL;
static uint32_t trino_t3_flags = 0;
static int trino_t3_mode = 0;
// The same for trino_3d_water: the layers TMEM holds and their flags
// (trino_w_sp NULL once anything else loaded TMEM) and whether the render
// mode is still the water's. A frame's water draws then upload their layers
// once; the detail layer's scroll changes every frame, and moving it is a
// SET_TILE_SIZE, not an upload (~0.25 ms of CPU per frame, issue #69).
static void* trino_w_sp = NULL;
static void* trino_w_dt = NULL;
static uint32_t trino_w_flags = 0;
static int trino_w_mode = 0;

// Standard render mode + magic-square RGB dither into the 16-bit
// framebuffer (libdragon's standard mode disables dithering).
static void trino_mode_standard(void)
{
    if (trino_rsp_tex_pending) {
        rdpq_sync_load();
        rdpq_sync_tile();
        trino_rsp_tex_pending = 0;
    }
    if (trino_rsp_pending) {
        rdpq_sync_pipe();
        trino_rsp_pending = 0;
    }
    trino_mode_3d = 0;
    trino_2d_ready = 0;
    trino_fill_ready = 0;
    trino_t3_mode = 0;
    trino_w_mode = 0;
    rdpq_set_mode_standard();
    rdpq_mode_dithering(DITHER_SQUARE_NONE);
}

void trino_init(void)
{
    debug_init_isviewer();
    debug_init_usblog();
    // The VI's dedither ("restore") filter rebuilds smooth color from the
    // RDP's magic-square dither (see trino_mode_standard). Coverage is
    // always full (standard mode zaps it), so the VI AA/divot stages that
    // come with this option leave pixels untouched. PC emulates the same
    // pair (PcRenderer: quantize_5551 + fs_vi_dedither).
    display_init(RESOLUTION_320x240, DEPTH_16_BPP, 2, GAMMA_NONE,
                 FILTERS_RESAMPLE_ANTIALIAS_DEDITHER);
    rdpq_init();
    joypad_init();
    timer_init();
    dfs_init(DFS_DEFAULT_LOCATION);
    audio_init(44100, 4);
    mixer_init(16);
    trino_check_trifmt();
    trino_rsp_id = rspq_overlay_register(&rsp_trino);
    trino_register_color_overlay();
    trino_register_tex_overlay();
    trino_register_water_overlay();
    register_VI_handler(trino_on_vi);
}

// Self-test stall watchdog (trino_watchdog_start). A self-test that stops
// printing is either stuck on the console (a CPU waiting on the RSP or the
// RDP) or a frozen emulator. While a test runs, a timer interrupt checks
// every 4 s of emulated time that a frame began or a line was logged since
// the last check; if not, it prints TRINO_STALL with where the CPU waits
// (trino_phase) and the RSP/RDP status registers. A stall that prints
// nothing at all is the emulator.
static volatile uint32_t trino_progress = 0;
static volatile const char* trino_phase = "boot";
static uint32_t trino_watch_seen = 0xFFFFFFFFu;
static int trino_watch_reports = 0;

#define TRINO_DP_REG(n) (*(volatile uint32_t*)(0xA4100000u + (n)))

static void trino_watch_tick(int ovfl)
{
    (void)ovfl;
    uint32_t p = trino_progress;
    if (p != trino_watch_seen) {
        trino_watch_seen = p;
        trino_watch_reports = 0;
        return;
    }
    if (trino_watch_reports >= 3) return;
    trino_watch_reports++;
    debugf("TRINO_STALL: no frame or log for %d s, waiting in %s; SP_STATUS=%08lx "
           "DP_START=%08lx DP_END=%08lx DP_CURRENT=%08lx DP_STATUS=%08lx\n",
           4 * trino_watch_reports, (const char*)trino_phase,
           (unsigned long)*SP_STATUS, (unsigned long)TRINO_DP_REG(0x0),
           (unsigned long)TRINO_DP_REG(0x4), (unsigned long)TRINO_DP_REG(0x8),
           (unsigned long)TRINO_DP_REG(0xC));
}

void trino_watchdog_start(void)
{
    static int started = 0;
    if (started) return;
    started = 1;
    new_timer(TIMER_TICKS(4000000), TF_CONTINUOUS, trino_watch_tick);
}

void trino_log(const char* msg)
{
    trino_progress++;
    debugf("%s", msg);
}

// Rust panic lands here: report over ISViewer/USB and halt.
void trino_panic(const char* msg)
{
    debugf("TRINO_PANIC: %s\n", msg);
    console_init();
    console_set_debug(false);
    printf("TRINO PANIC\n%s\n", msg);
    console_render();
    while (1) {}
}

uint32_t trino_ticks_us(void)
{
    // Exact conversion: TICKS_PER_SECOND / 1000000 is 46.875, and the
    // integer quotient (46) made every timing read 1.9% high.
    return (uint32_t)TIMER_MICROS_LL(timer_ticks());
}

uint32_t trino_vblank_count(void)
{
    return trino_vblanks;
}

// ---------------------------------------------------------------------------
// Frame

// Hardware z-buffer for the 3D triangles (allocated on first frame).
static surface_t trino_zbuf;
static int trino_zbuf_ready = 0;

// Last frame's framebuffer, for the trino_debug_pixel test hook.
static surface_t* trino_last_fb = NULL;

void trino_frame_begin(uint32_t rgba8888)
{
    trino_progress++;
    trino_phase = "display_get";
    surface_t* fb = display_get();
    trino_phase = "frame";
    trino_last_fb = fb;
    if (!trino_zbuf_ready) {
        trino_zbuf = surface_alloc(FMT_RGBA16, display_get_width(), display_get_height());
        trino_zbuf_ready = 1;
    }
    trino_mode_3d = 0;
    trino_2d_ready = 0;
    trino_tmem_owner = NULL;
    trino_t3_sp = NULL;
    // TMEM keeps the water's layers across frames (every upload here
    // clears trino_w_sp), so only its mode is set again.
    trino_w_mode = 0;
    trino_t3_mode = 0;
    rdpq_attach(fb, &trino_zbuf);
    rdpq_clear(RGBA32((rgba8888 >> 24) & 0xFF, (rgba8888 >> 16) & 0xFF,
                      (rgba8888 >> 8) & 0xFF, 255));
    rdpq_clear_z(0xFFFC);
}

void trino_frame_end(void)
{
    if (trino_rsp_pending) {
        rdpq_sync_pipe();
        trino_rsp_pending = 0;
    }
    trino_mode_3d = 0;
    trino_2d_ready = 0;
    trino_phase = "frame_end (rdpq_detach_show)";
    rdpq_detach_show();
    trino_phase = "between frames";
}

// Profiling: wait until the RSP and the RDP have finished everything queued,
// so a measured frame includes the work it handed them (with RSP T&L that
// work no longer shows up as CPU time).
void trino_drain(void)
{
    trino_phase = "drain (rspq_wait)";
    rdpq_fence();
    rspq_wait();
    trino_phase = "frame";
}

// ---------------------------------------------------------------------------
// Sprites

void* trino_sprite_load(const char* dfs_path)
{
    return sprite_load(dfs_path);
}

// Free a sprite (its asset group unloaded, issue #56). The renderer calls
// it frames after the last draw that could use it (the RDP reads texels
// from RDRAM while a frame drains); the TMEM caches forget it first, so a
// sprite loaded later at the same address is uploaded again.
void trino_sprite_free(void* sprite)
{
    if (trino_tmem_owner == sprite) trino_tmem_owner = NULL;
    if (trino_t3_sp == sprite || trino_t3_dt == sprite) { trino_t3_sp = NULL; trino_t3_dt = NULL; }
    if (trino_w_sp == sprite || trino_w_dt == sprite) { trino_w_sp = NULL; trino_w_dt = NULL; }
    sprite_free((sprite_t*)sprite);
}

uint32_t trino_sprite_size(void* sprite)
{
    sprite_t* s = sprite;
    return ((uint32_t)s->width << 16) | (uint32_t)s->height;
}

// Layout mirrored by TrinoBlit in ../src/ffi.rs. Fields only, no bitfields.
typedef struct {
    float x, y;
    float scale_x, scale_y;
    float theta;          // radians
    uint32_t flip_x;      // 0/1
    uint32_t flip_y;      // 0/1
    uint32_t tint;        // RGBA8888; 0xFFFFFFFF = no tint
    int32_t src_x, src_y; // source sub-rect (sprite pixels)
    int32_t src_w, src_h; // 0 = whole sprite
} trino_blit_t;

// 2D mode for a blit or a text run, with the tint (0xFFFFFFFF = none).
static void trino_2d_setup(uint32_t tint)
{
    if (!trino_2d_ready || trino_mode_3d || trino_rsp_pending) {
        trino_mode_standard();
        rdpq_mode_alphacompare(1);
        trino_2d_ready = 1;
        trino_2d_tint = 0xFFFFFFFFu; // standard mode's combiner: texture only
    }
    if (tint != trino_2d_tint) {
        if (tint == 0xFFFFFFFFu) {
            rdpq_mode_combiner(RDPQ_COMBINER_TEX);
        } else {
            rdpq_set_prim_color(RGBA32((tint >> 24) & 0xFF, (tint >> 16) & 0xFF,
                                       (tint >> 8) & 0xFF, tint & 0xFF));
            if (trino_2d_tint == 0xFFFFFFFFu) {
                rdpq_mode_combiner(RDPQ_COMBINER1((TEX0, 0, PRIM, 0), (TEX0, 0, PRIM, 0)));
            }
        }
        trino_2d_tint = tint;
    }
}

void trino_sprite_blit(void* sprite, const trino_blit_t* p)
{
    trino_2d_setup(p->tint);
    trino_tmem_owner = NULL;
    trino_t3_sp = NULL;
    trino_w_sp = NULL;
    rdpq_blitparms_t parms = {
        .s0 = p->src_x,
        .t0 = p->src_y,
        .width = p->src_w,
        .height = p->src_h,
        .scale_x = p->scale_x,
        .scale_y = p->scale_y,
        .theta = p->theta,
        .flip_x = p->flip_x != 0,
        .flip_y = p->flip_y != 0,
    };
    rdpq_sprite_blit((sprite_t*)sprite, p->x, p->y, &parms);
}

// Solid rectangle (fill_rect): flat PRIM combiner, alpha-blended when
// translucent, no texture. xy / wh = x << 16 | y (signed 16-bit, whole
// pixels), color = RGBA8888. Consecutive fills of the same color reuse the
// mode (the blit cache is invalidated: a blit after a fill re-sets its mode).
static uint32_t trino_fill_color = 0;
static int trino_fill_blend = 0;
static __attribute__((noinline)) void trino_fill_rect_body(uint32_t xy, uint32_t wh, uint32_t color)
{
    if (!trino_fill_ready || trino_mode_3d || trino_rsp_pending) {
        trino_mode_standard(); // no blending
        rdpq_mode_combiner(RDPQ_COMBINER_FLAT);
        trino_fill_ready = 1;
        trino_fill_blend = 0;
        trino_fill_color = ~color; // force the color below
    }
    // A blit after this fill must set its own mode again.
    trino_2d_ready = 0;
    if (color != trino_fill_color) {
        int a = color & 0xFF;
        int blend = a < 255;
        if (blend != trino_fill_blend) {
            rdpq_mode_blender(blend ? RDPQ_BLENDER_MULTIPLY : 0);
            trino_fill_blend = blend;
        }
        rdpq_set_prim_color(RGBA32((color >> 24) & 0xFF, (color >> 16) & 0xFF,
                                   (color >> 8) & 0xFF, a));
        trino_fill_color = color;
    }
    int32_t x = (int16_t)(xy >> 16), y = (int16_t)(xy & 0xFFFF);
    int32_t w = (int16_t)(wh >> 16), h = (int16_t)(wh & 0xFFFF);
    if (w > 0 && h > 0) {
        rdpq_fill_rectangle(x, y, x + w, y + h);
    }
}

// Text with the whole font atlas resident in TMEM: one upload per string,
// then one texture rectangle per glyph (trino_text_glyph) — a per-glyph
// rdpq_sprite_blit re-uploads texels (and a CI palette) every time, ~50 us
// each. Returns 1 when the atlas fits (I4/I8/RGBA16: 4 KB; CI: 2 KB, the
// palette takes the rest) and was uploaded, else 0 (blit per glyph).
uint32_t trino_text_begin(void* sprite, uint32_t tint)
{
    sprite_t* sp = (sprite_t*)sprite;
    tex_format_t fmt = sprite_get_format(sp);
    int row = (TEX_FORMAT_PIX2BYTES(fmt, sp->width) + 7) & ~7;
    int limit = (fmt == FMT_CI4 || fmt == FMT_CI8) ? 2048 : 4096;
    if (row * sp->height > limit) {
        return 0;
    }
    trino_2d_setup(tint);
    if (trino_tmem_owner != sprite) {
        rdpq_sprite_upload(TILE0, sp, NULL);
        trino_tmem_owner = sprite;
        trino_t3_sp = NULL;
        trino_w_sp = NULL;
    }
    return 1;
}

// A run of glyphs of the resident atlas (after trino_text_begin): `n`
// records of 3 words each, (xy = screen x << 16 | y, signed 16-bit whole
// pixels; st = atlas s << 16 | t; wh = w << 16 | h), at
// integer scale `scale`. One call per string instead of one per glyph, and
// on-screen glyphs skip libdragon's general rectangle path (two divisions
// and four clip tests per glyph): at an integer scale the texture steps are
// the constants 1024 / scale, which is what that path computes for every
// glyph, so the RDP gets the same words. Glyphs that cross the screen's
// top or left edge take the general path.
static __attribute__((noinline)) void trino_text_run_body(const uint32_t* g, uint32_t n, uint32_t scale)
{
    int32_t k = (int32_t)scale;
    int32_t step = (32 << 7) / (4 * k);
    uint32_t steps = ((uint32_t)step << 16) | ((uint32_t)step & 0xFFFF);
    extern void __rdpq_texture_rectangle(uint32_t w0, uint32_t w1, uint32_t w2, uint32_t w3);
    for (uint32_t i = 0; i < n; i++, g += 3) {
        int32_t x = (int16_t)(g[0] >> 16), y = (int16_t)(g[0] & 0xFFFF);
        int32_t s0 = (int32_t)(g[1] >> 16), t0 = (int32_t)(g[1] & 0xFFFF);
        int32_t w = (int32_t)(g[2] >> 16), h = (int32_t)(g[2] & 0xFFFF);
        int32_t x0 = x * 4, y0 = y * 4, x1 = (x + w * k) * 4, y1 = (y + h * k) * 4;
        if (__builtin_expect(x0 >= 0 && y0 >= 0 && x1 <= 1024 * 4 - 1 && y1 <= 1024 * 4 - 1
                             && w > 0 && h > 0, 1)) {
            __rdpq_texture_rectangle(
                ((uint32_t)x1 << 12) | (uint32_t)y1,
                ((uint32_t)TILE0 << 24) | ((uint32_t)x0 << 12) | (uint32_t)y0,
                ((uint32_t)(s0 * 32) << 16) | ((uint32_t)(t0 * 32) & 0xFFFF),
                steps);
        } else {
            __rdpq_texture_rectangle_scaled_fx(TILE0, x0, y0, x1, y1,
                                               s0 * 32, t0 * 32, (s0 + w) * 32, (t0 + h) * 32);
        }
    }
}

// ---------------------------------------------------------------------------
// 3D triangles: the engine transforms and lights on the CPU
// (trino_core::render3d); the RDP only rasterizes gouraud-shaded tris.

// Call once before a batch of trino_tri (shade combiner + hardware z-test).
void trino_3d_begin(void)
{
    trino_mode_standard();
    rdpq_mode_combiner(RDPQ_COMBINER_SHADE);
    rdpq_mode_zbuf(true, true);
    trino_mode_3d = 1;
}

// RSP T&L: one static mesh draw (rsp_trino.S TrinoCmd_Draw). `w` holds the
// overlay's DRAW layout, 27 words; word 0's low 24 bits are the flags.
#define TRINO_RSP_DRAW_WORDS 27
void trino_rsp_draw(const uint32_t* w)
{
    if (!trino_mode_3d) trino_3d_begin();
    rspq_write_t c = rspq_write_begin(trino_rsp_id, 0x1, TRINO_RSP_DRAW_WORDS);
    rspq_write_arg(&c, w[0] & 0xFFFFFF);
    for (int i = 1; i < TRINO_RSP_DRAW_WORDS; i++) rspq_write_arg(&c, w[i]);
    rspq_write_end(&c);
    trino_rsp_pending = 1;
}

// Uncached RDRAM for data the RSP reads by DMA (no cache writeback needed),
// 16-byte aligned; and its physical address for the overlay.
void* trino_alloc_uncached(uint32_t size)
{
    return malloc_uncached(size);
}

// Free what trino_alloc_uncached returned (an RSP mesh copy whose model
// unloaded, issue #56; the renderer waits frames before calling it).
void trino_free_uncached(void* p)
{
    free_uncached(p);
}

uint32_t trino_phys_addr(const void* p)
{
    return PhysicalAddr(p);
}

// RDRAM in bytes: 4 MiB, or 8 MiB with the Expansion Pak (the boot's
// hardware tier, issue #54; the Rust side applies libdragon's
// is_memory_expanded threshold, src/hw.rs).
uint32_t trino_memory_size(void)
{
    return (uint32_t)get_memory_size();
}

// Heap use (bytes): the self-test reports it next to the stack watermark.
uint32_t trino_heap_used(void)
{
    heap_stats_t s;
    sys_get_heap_stats(&s);
    return (uint32_t)s.used;
}

uint32_t trino_heap_total(void)
{
    heap_stats_t s;
    sys_get_heap_stats(&s);
    return (uint32_t)s.total;
}

// The heap's top free block (GitHub #110): newlib's top chunk plus what
// sbrk has not handed out yet. Free bytes say little on a fragmented 4 MB
// heap; big allocations come from here. Allocates nothing.
uint32_t trino_heap_top_free(void)
{
    extern char *__heap_end;
    extern char *__heap_top;
    struct mallinfo m = mallinfo();
    return (uint32_t)m.keepcost + (uint32_t)(__heap_top - __heap_end);
}

// pts: 6 floats (x0,y0,x1,y1,x2,y2) in screen pixels;
// colors: 12 bytes (r,g,b,a per vertex);
// zs: 3 floats, normalized depth 0..1 (0 = near plane).
// Vertex layout is driven by TRIFMT_ZBUF_SHADE's own offsets so a libdragon
// layout change is a behavior-preserving recompile, not silent corruption.
static __attribute__((noinline)) void trino_tri_body(const float* pts, const uint8_t* colors, const float* zs)
{
    // Sized past any TRIFMT layout: the offsets come from libdragon.
    float v[3][16];
    for (int i = 0; i < 3; i++) {
        v[i][TRIFMT_ZBUF_SHADE.pos_offset + 0] = pts[i * 2 + 0];
        v[i][TRIFMT_ZBUF_SHADE.pos_offset + 1] = pts[i * 2 + 1];
        v[i][TRIFMT_ZBUF_SHADE.z_offset] = zs[i];
        v[i][TRIFMT_ZBUF_SHADE.shade_offset + 0] = colors[i * 4 + 0] / 255.0f;
        v[i][TRIFMT_ZBUF_SHADE.shade_offset + 1] = colors[i * 4 + 1] / 255.0f;
        v[i][TRIFMT_ZBUF_SHADE.shade_offset + 2] = colors[i * 4 + 2] / 255.0f;
        v[i][TRIFMT_ZBUF_SHADE.shade_offset + 3] = colors[i * 4 + 3] / 255.0f;
    }
    rdpq_triangle(&TRIFMT_ZBUF_SHADE, v[0], v[1], v[2]);
}

// RSP overlay milestone 1: one fixed triangle assembled by the overlay
// (shade + z), drawn in the current 3D mode (call trino_3d_begin first).
void trino_rsp_test_tri(void)
{
    rspq_write(trino_rsp_id, 0x0, 0xCD00);
}

// Startup self-check of the vertex layouts the trino_tri* buffers assume.
static void trino_check_trifmt(void)
{
    const rdpq_trifmt_t* f = &TRIFMT_ZBUF_SHADE_TEX;
    int top = f->pos_offset + 2;
    if (f->z_offset + 1 > top) top = f->z_offset + 1;
    if (f->tex_offset + 3 > top) top = f->tex_offset + 3;
    if (f->shade_offset + 4 > top) top = f->shade_offset + 4;
    if (top > 16) {
        debugf("TRINO_PANIC: TRIFMT_ZBUF_SHADE_TEX needs %d floats\n", top);
    }
}

// Textured 3D: select `sprite` as the texture of the next trino_tri_tex
// calls. One texture per draw lives in TMEM (TILE0, wrapping both ways);
// the combiner modulates it by the lit vertex color (TEX0 * SHADE), with
// perspective correction and the RDP's native 3-point filter.
static float trino_tex_w = 1.0f, trino_tex_h = 1.0f;

// clamp != 0: billboard mode — no tiling (a sprite region must not bleed
// into its neighbours) and a half-alpha cutout for soft-edged sprites.
void trino_3d_texture(void* sprite, uint32_t clamp)
{
    trino_tmem_owner = NULL;
    trino_t3_sp = NULL;
    trino_w_sp = NULL;
    sprite_t* sp = (sprite_t*)sprite;
    trino_mode_standard();
    rdpq_mode_combiner(RDPQ_COMBINER_TEX_SHADE);
    rdpq_mode_zbuf(true, true);
    rdpq_mode_persp(true);
    rdpq_mode_filter(FILTER_BILINEAR);
    rdpq_mode_alphacompare(clamp ? 128 : 1);
    float repeats = clamp ? 1.0f : REPEAT_INFINITE;
    rdpq_texparms_t parms = {
        .s.repeats = repeats,
        .t.repeats = repeats,
    };
    rdpq_sprite_upload(TILE0, sp, &parms);
    trino_tex_w = (float)sp->width;
    trino_tex_h = (float)sp->height;
}

// TMEM bytes of a sprite's texels (rows padded to 8 bytes, as rdpq loads
// them); -1 for palette formats, whose TLUT takes TMEM's upper half.
static int trino_tmem_bytes(sprite_t* sp)
{
    tex_format_t f = sprite_get_format(sp);
    if (f == FMT_CI4 || f == FMT_CI8) return -1;
    return ((TEX_FORMAT_PIX2BYTES(f, sp->width) + 7) & ~7) * sp->height;
}

// log2 of a power of two (the tile sizes; 0 for anything else).
static int trino_log2(uint32_t v)
{
    return v ? __builtin_ctz(v) : 0;
}

#ifndef TRINO_T3_CACHE
#define TRINO_T3_CACHE 1
#endif
// trino_3d_texture_ex's render mode: TEX0 * SHADE (times TEX1 with a
// detail layer), z-buffered, perspective-correct, filtered by `flags`.
static void trino_t3_texture_mode(int detail, uint32_t flags)
{
    trino_mode_standard();
    if (detail) {
        rdpq_mode_combiner(RDPQ_COMBINER2((TEX0, 0, SHADE, 0), (TEX0, 0, SHADE, 0),
                                          (COMBINED, 0, TEX1, 0), (0, 0, 0, COMBINED)));
    } else {
        rdpq_mode_combiner(RDPQ_COMBINER_TEX_SHADE);
    }
    rdpq_mode_zbuf(true, true);
    rdpq_mode_persp(true);
    rdpq_mode_filter((flags & 2) ? FILTER_POINT : FILTER_BILINEAR);
    rdpq_mode_alphacompare((flags & 1) ? 128 : 1);
    trino_t3_mode = 1;
}

// Textured 3D with a filter and an optional detail layer (see ffi.rs):
// flags bit 0 clamp, bit 1 point filter, bits 4..7 the detail's repeats
// per base repeat (log2). The detail tile reads the same texture
// coordinates as TILE0, shifted so it tiles 2^n times per base repeat
// whatever the two sizes; the 2-cycle combiner multiplies it in:
// (TEX0 * SHADE) then (COMBINED * TEX1).
static __attribute__((noinline)) uint32_t trino_3d_texture_ex_body(void* sprite, void* detail, uint32_t flags)
{
    sprite_t* sp = (sprite_t*)sprite;
    sprite_t* dt = (sprite_t*)detail;
    uint32_t clamp = flags & 1;
    int shift = (flags >> 4) & 15;
    if (dt) {
        int a = trino_tmem_bytes(sp), b = trino_tmem_bytes(dt);
        if (a < 0 || b < 0 || a + b > 4096) dt = NULL;
    }
    int resident = TRINO_T3_CACHE && trino_t3_sp == sp && trino_t3_dt == dt && trino_t3_flags == flags;
    if (resident && trino_t3_mode) return dt != NULL;
    if (resident) {
        // Same texels, another mode in between (an untextured RSP draw):
        // only the mode again, with the palette lookup the upload had set
        // for a CI texture (the standard mode clears it: CI crates drew
        // black).
        trino_t3_texture_mode(dt != NULL, flags);
        rdpq_mode_tlut(rdpq_tlut_from_format(sprite_get_format(sp)));
        return dt != NULL;
    }
    trino_tmem_owner = NULL;
    trino_w_sp = NULL;
    trino_t3_texture_mode(dt != NULL, flags);
    trino_t3_sp = sp;
    trino_t3_dt = dt;
    trino_t3_flags = flags;
    float repeats = clamp ? 1.0f : REPEAT_INFINITE;
    rdpq_texparms_t parms = {
        .s.repeats = repeats,
        .t.repeats = repeats,
    };
    if (dt) {
        rdpq_texparms_t dparms = {
            .s.repeats = REPEAT_INFINITE,
            .t.repeats = REPEAT_INFINITE,
            .s.scale_log = trino_log2(sp->width) - trino_log2(dt->width) - shift,
            .t.scale_log = trino_log2(sp->height) - trino_log2(dt->height) - shift,
        };
        rdpq_tex_multi_begin();
        rdpq_sprite_upload(TILE0, sp, &parms);
        rdpq_sprite_upload(TILE1, dt, &dparms);
        rdpq_tex_multi_end();
    } else {
        rdpq_sprite_upload(TILE0, sp, &parms);
    }
    trino_tex_w = (float)sp->width;
    trino_tex_h = (float)sp->height;
    return dt != NULL;
}

// Water (ADR 0015, ffi.rs): translucent, z-compared but not written, alpha
// from the shade (the T&L's vertex alpha with its Fresnel term). With both
// layers: the 2-cycle combiner blends them half and half (ENV alpha 0.5)
// and multiplies by the shade; TILE1 reads the base's coordinates shifted
// like a detail tile and translated by `scroll` (u << 16 | v, 1/65536 of
// its repeat), so it flows on its own. The pair must fit TMEM and neither
// may be a palette, or the layer is dropped.
// The detail tile's translation for `scroll` (see trino_3d_water_body).
static float trino_water_translate(uint32_t v, int size)
{
    return (1.0f - (float)v / 65536.0f) * (float)size;
}

static __attribute__((noinline)) uint32_t trino_3d_water_body(void* sprite, void* detail, uint32_t flags, uint32_t scroll)
{
    trino_tmem_owner = NULL;
    trino_t3_sp = NULL;
    sprite_t* sp = (sprite_t*)sprite;
    sprite_t* dt = sp ? (sprite_t*)detail : NULL;
    int shift = (flags >> 4) & 15;
    if (dt) {
        int a = trino_tmem_bytes(sp), b = trino_tmem_bytes(dt);
        if (a < 0 || b < 0 || a + b > 4096) dt = NULL;
    }
    int resident = sp && trino_w_sp == sp && trino_w_dt == dt && trino_w_flags == flags;
    if (!resident || !trino_w_mode) {
        trino_mode_standard();
        if (dt) {
            rdpq_set_env_color(RGBA32(0, 0, 0, 128));
            rdpq_mode_combiner(RDPQ_COMBINER2((TEX1, TEX0, ENV_ALPHA, TEX0), (0, 0, 0, SHADE),
                                              (COMBINED, 0, SHADE, 0), (0, 0, 0, SHADE)));
        } else if (sp) {
            rdpq_mode_combiner(RDPQ_COMBINER1((TEX0, 0, SHADE, 0), (0, 0, 0, SHADE)));
        } else {
            rdpq_mode_combiner(RDPQ_COMBINER_SHADE);
        }
        rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY);
        rdpq_mode_zbuf(true, false);
        rdpq_mode_alphacompare(0);
        trino_mode_3d = 0;
        if (!sp) return 0;
        rdpq_mode_persp(true);
        rdpq_mode_filter((flags & 2) ? FILTER_POINT : FILTER_BILINEAR);
        trino_w_mode = 1;
    }
    if (resident) {
        // Same layers in TMEM: move the detail tile's origin only, as the
        // upload below sets it (tex_loader's s0fx = translate * 4; the
        // repeats are infinite, so the end moves by as much). The RSP's
        // textured triangles in flight read the tile: sync first.
        if (dt) {
            int s0 = (int)(trino_water_translate(scroll >> 16, dt->width) * 4);
            int t0 = (int)(trino_water_translate(scroll & 0xFFFF, dt->height) * 4);
            rdpq_sync_tile();
            rdpq_set_tile_size_fx(TILE1, s0, t0, dt->width * 4 + s0, dt->height * 4 + t0);
        }
        return dt != NULL;
    }
    rdpq_texparms_t parms = {
        .s.repeats = REPEAT_INFINITE,
        .t.repeats = REPEAT_INFINITE,
    };
    if (dt) {
        // The RDP subtracts a tile's origin from its coordinates: the
        // texel at uv * scale + offset is at origin = (1 - offset) * size.
        rdpq_texparms_t dparms = {
            .s.repeats = REPEAT_INFINITE,
            .t.repeats = REPEAT_INFINITE,
            .s.scale_log = trino_log2(sp->width) - trino_log2(dt->width) - shift,
            .t.scale_log = trino_log2(sp->height) - trino_log2(dt->height) - shift,
            .s.translate = trino_water_translate(scroll >> 16, dt->width),
            .t.translate = trino_water_translate(scroll & 0xFFFF, dt->height),
        };
        rdpq_tex_multi_begin();
        rdpq_sprite_upload(TILE0, sp, &parms);
        rdpq_sprite_upload(TILE1, dt, &dparms);
        rdpq_tex_multi_end();
    } else {
        rdpq_sprite_upload(TILE0, sp, &parms);
    }
    trino_w_sp = sp;
    trino_w_dt = dt;
    trino_w_flags = flags;
    trino_tex_w = (float)sp->width;
    trino_tex_h = (float)sp->height;
    return dt != NULL;
}

// Shadows (ADR 0025): blob and planar shadow triangles blend the shade's
// color over the 3D by its alpha, z-tested and never written, like water
// (the core moved them a few z steps in front of their receiver).
// `texels`: the blob's `size`² I8 falloff in uncached RDRAM, its alpha
// (TEX0's alpha is its intensity), for trino_tri_tex; NULL: planar
// shadows, the shade alone, for trino_tri.
void trino_3d_shadow(const void* texels, uint32_t size)
{
    trino_tmem_owner = NULL;
    trino_t3_sp = NULL;
    trino_w_sp = NULL;
    trino_mode_standard();
    rdpq_mode_blender(RDPQ_BLENDER_MULTIPLY);
    rdpq_mode_zbuf(true, false);
    rdpq_mode_alphacompare(0);
    if (!texels) {
        rdpq_mode_combiner(RDPQ_COMBINER_SHADE);
        return;
    }
    rdpq_mode_combiner(RDPQ_COMBINER1((0, 0, 0, SHADE), (TEX0, 0, SHADE, 0)));
    rdpq_mode_persp(true);
    rdpq_mode_filter(FILTER_BILINEAR);
    // Clamped (no repeats): the falloff's outer texels are 0.
    surface_t surf = surface_make_linear((void*)texels, FMT_I8, size, size);
    rdpq_tex_upload(TILE0, &surf, NULL);
    trino_tex_w = (float)size;
    trino_tex_h = (float)size;
}

// v: 3 vertices x 10 floats (x, y, z, u, v, w, r, g, b, a), colors 0..1,
// u/v normalized (scaled to texels here), w = view depth (> 0).
void trino_tri_tex(const float* in)
{
    // Sized past any TRIFMT layout: the offsets come from libdragon.
    float v[3][16];
    const rdpq_trifmt_t* fmt = &TRIFMT_ZBUF_SHADE_TEX;
    for (int i = 0; i < 3; i++) {
        const float* s = in + i * 10;
        v[i][fmt->pos_offset + 0] = s[0];
        v[i][fmt->pos_offset + 1] = s[1];
        v[i][fmt->z_offset] = s[2];
        v[i][fmt->tex_offset + 0] = s[3] * trino_tex_w;
        v[i][fmt->tex_offset + 1] = s[4] * trino_tex_h;
        v[i][fmt->tex_offset + 2] = 1.0f / s[5];
        v[i][fmt->shade_offset + 0] = s[6];
        v[i][fmt->shade_offset + 1] = s[7];
        v[i][fmt->shade_offset + 2] = s[8];
        v[i][fmt->shade_offset + 3] = s[9];
    }
    rdpq_triangle(fmt, v[0], v[1], v[2]);
}

// Test hook: RGBA8888 of the last frame at (x, y); 0 if unavailable. Waits
// for the RDP, then reads the RGBA5551 framebuffer through the uncached
// segment (the RDP wrote it behind the CPU cache).
uint32_t trino_debug_pixel(uint32_t x, uint32_t y)
{
    if (!trino_last_fb || x >= trino_last_fb->width || y >= trino_last_fb->height) return 0;
    rspq_wait();
    const uint8_t* base = UncachedAddr(trino_last_fb->buffer);
    uint16_t px = *(const uint16_t*)(base + y * trino_last_fb->stride + x * 2);
    uint32_t r = (px >> 11) & 31, g = (px >> 6) & 31, b = (px >> 1) & 31;
    r = (r << 3) | (r >> 2);
    g = (g << 3) | (g >> 2);
    b = (b << 3) | (b >> 2);
    return (r << 24) | (g << 16) | (b << 8) | 0xFF;
}

// ---------------------------------------------------------------------------
// Input — bits 0..11 match trino_core::input::Button discriminants;
// bits 16..19 are the raw C buttons (up, down, left, right), which Rust
// turns into `InputState::right_stick` (C-left/C-down also stay X/Y).

static uint32_t trino_pack_buttons(joypad_buttons_t b);

uint32_t trino_joypad_buttons(void)
{
    joypad_poll();
    return trino_pack_buttons(joypad_get_buttons_held(JOYPAD_PORT_1));
}

// The FPU's control/status word (issue #67, the `replay` step's log):
// libdragon's flush-to-zero (FS) and exception enables decide how
// subnormals and NaNs behave, which the determinism docs cite.
uint32_t trino_fpu_control(void)
{
    return C1_FCR31();
}

// Local multiplayer (issue #67): one poll a frame, then each port.
// Returns bit `p` for every port holding an N64 controller.
uint32_t trino_joypad_poll(void)
{
    joypad_poll();
    uint32_t mask = 0;
    JOYPAD_PORT_FOREACH(port) {
        if (joypad_get_style(port) == JOYPAD_STYLE_N64) mask |= 1u << port;
    }
    return mask;
}

uint32_t trino_joypad_port_buttons(uint32_t port)
{
    if (port >= JOYPAD_PORT_COUNT) return 0;
    return trino_pack_buttons(joypad_get_buttons_held((joypad_port_t)port));
}

uint32_t trino_joypad_port_stick(uint32_t port)
{
    if (port >= JOYPAD_PORT_COUNT) return 0;
    joypad_inputs_t in = joypad_get_inputs((joypad_port_t)port);
    uint16_t x = (uint16_t)(int16_t)in.stick_x;
    uint16_t y = (uint16_t)(int16_t)in.stick_y;
    return ((uint32_t)x << 16) | y;
}

static uint32_t trino_pack_buttons(joypad_buttons_t b)
{
    uint32_t out = 0;
    if (b.a) out |= 1u << 0;        // A
    if (b.b) out |= 1u << 1;        // B
    if (b.c_left) out |= 1u << 2;   // X
    if (b.c_down) out |= 1u << 3;   // Y
    if (b.l) out |= 1u << 4;        // L
    if (b.r) out |= 1u << 5;        // R
    if (b.start) out |= 1u << 6;    // Start
    if (b.z) out |= 1u << 7;        // Select
    if (b.d_up) out |= 1u << 8;
    if (b.d_down) out |= 1u << 9;
    if (b.d_left) out |= 1u << 10;
    if (b.d_right) out |= 1u << 11;
    if (b.c_up) out |= 1u << 16;
    if (b.c_down) out |= 1u << 17;
    if (b.c_left) out |= 1u << 18;
    if (b.c_right) out |= 1u << 19;
    return out;
}

// Packed stick: high 16 bits = x, low 16 = y, as signed 16-bit each.
uint32_t trino_joypad_stick(void)
{
    joypad_inputs_t in = joypad_get_inputs(JOYPAD_PORT_1);
    uint16_t x = (uint16_t)(int16_t)in.stick_x;
    uint16_t y = (uint16_t)(int16_t)in.stick_y;
    return ((uint32_t)x << 16) | y;
}

// ---------------------------------------------------------------------------
// Audio

// What the audio holds and does, for `Audio::audio_stats` and the
// `audio codecs` self-test step (issue #53). Every wav64 streams from the
// ROM through its channel's sample buffer, so the heap the mixer allocates
// for those buffers (on a channel's first play) is the streaming memory;
// what `wav64_open` allocates (the struct, a VADPCM codebook, an Opus
// decoder) is resident. Both are measured as heap deltas
// (trino_heap_used).

static uint32_t trino_audio_resident;
static uint32_t trino_audio_stream_bytes;
static uint32_t trino_audio_underruns;
static uint32_t trino_audio_streamed;
static uint64_t trino_audio_ticks;
static uint32_t trino_audio_polls;
static uint32_t trino_audio_buffers;
static uint32_t trino_ch_pos[16];
extern int64_t __mixer_profile_rsp;

void* trino_wav_load(const char* dfs_path)
{
    uint32_t before = trino_heap_used();
    wav64_t* wav = malloc(sizeof(wav64_t));
    wav64_open(wav, dfs_path);
    trino_audio_resident += trino_heap_used() - before;
    return wav;
}

// Opus (wav64 compression level 3) links libopus. The shim's default does
// nothing and reports 0; `trino_opus.c`, linked by the bake only when an
// asset is Opus, enables it (and raises the channels' rate limit to the
// 48 kHz Opus plays at).
__attribute__((weak)) uint32_t trino_audio_opus_init(void)
{
    return 0;
}

// The wav64 each channel was last configured for, and whether it looped.
static void* trino_ch_wave[16];
static uint8_t trino_ch_loop[16];

// Close and free a wav64 (its asset group unloaded, issue #56): the
// channels playing it stop first, so the mixer never reads it again.
// Link and enable libdragon's level 2 or 3 decompressor (level 1 is always
// there): the index names each level mkasset used before any asset loads.
void trino_asset_compression(int level)
{
    if (level == 2) __asset_init_compression_lvl2();
    if (level == 3) __asset_init_compression_lvl3();
}

void trino_wav_free(void* wav)
{
    for (int ch = 0; ch < 16; ch++) {
        if (trino_ch_wave[ch] == wav) {
            mixer_ch_stop(ch);
            trino_ch_wave[ch] = NULL;
        }
    }
    uint32_t before = trino_heap_used();
    wav64_close((wav64_t*)wav);
    free(wav);
    uint32_t after = trino_heap_used();
    uint32_t freed = before > after ? before - after : 0;
    trino_audio_resident = trino_audio_resident > freed ? trino_audio_resident - freed : 0;
}
// Played for no time to make the mixer forget a channel's waveform.
static waveform_t trino_no_wave = {
    .name = "trino-reset", .bits = 16, .channels = 1, .frequency = 8000,
};

// `looped` loops the whole sound, as on the other platforms (the bake
// makes every length a multiple of 16, which libdragon wants of a VADPCM
// loop). The mixer keeps a channel's loop length while the same waveform
// replays on it, so a changed flag first plays the empty waveform there.
static __attribute__((noinline)) void trino_wav_play_body(void* wav, uint32_t channel, uint32_t looped)
{
    wav64_t* w = (wav64_t*)wav;
    looped = looped != 0;
    if (channel >= 16)
        return;
    if (trino_ch_wave[channel] == wav && trino_ch_loop[channel] != looped) {
        mixer_ch_play((int)channel, &trino_no_wave);
        mixer_ch_stop((int)channel);
    }
    wav64_set_loop(w, looped);
    trino_ch_wave[channel] = wav;
    trino_ch_loop[channel] = (uint8_t)looped;
    // A channel's first play allocates its sample buffer.
    uint32_t before = trino_heap_used();
    wav64_play(w, (int)channel);
    trino_audio_stream_bytes += trino_heap_used() - before;
    trino_ch_pos[channel] = 0;
}

void trino_channel_stop(uint32_t channel)
{
    mixer_ch_stop((int)channel);
}

void trino_mixer_set_vol(uint32_t vol_q16)
{
    mixer_set_vol(vol_q16 / 65536.0f);
}

// Per-channel left/right volume (pan + attenuation, music ducking).
// Volumes are 16.16-free fixed point (65536 = 1.0) on purpose: with an
// integer first argument, o32 (Rust) passes following floats in integer
// registers while o64 (libdragon) uses FP registers — scalar floats after
// an int would arrive as garbage. Integers and pointers are always safe.
static __attribute__((noinline)) void trino_ch_volume_body(uint32_t channel, uint32_t lvol_q16, uint32_t rvol_q16)
{
    mixer_ch_set_vol((int)channel, lvol_q16 / 65536.0f, rvol_q16 / 65536.0f);
}

int32_t trino_ch_playing(uint32_t channel)
{
    return mixer_ch_playing((int)channel) ? 1 : 0;
}

// Bit n set = channel n plays (one call per frame instead of 16).
uint32_t trino_audio_playing_mask(void)
{
    uint32_t mask = 0;
    for (int ch = 0; ch < 16; ch++)
        if (mixer_ch_playing(ch))
            mask |= 1u << ch;
    return mask;
}

// Feed the AI every free buffer. libdragon keeps 4 of 1/25 s; finding all
// of them free means the AI played everything it had and went silent: an
// underrun (the first poll, before anything was queued, doesn't count).
// The time spent here is the audio's whole frame cost — mixer_poll waits
// for the RSP (mixing, VADPCM decoding) and decodes Opus on the CPU.
void trino_audio_poll(void)
{
    uint32_t t0 = TICKS_READ();
    int written = 0;
    while (audio_can_write()) {
        int16_t* buf = audio_write_begin();
        mixer_poll(buf, audio_get_buffer_length());
        audio_write_end();
        written++;
    }
    if (written >= 4 && trino_audio_polls > 0)
        trino_audio_underruns++;
    trino_audio_buffers += written;
    trino_audio_polls++;
    // Progress of every playing channel, in source sample frames.
    for (int ch = 0; ch < 16; ch++) {
        if (!mixer_ch_playing(ch))
            continue;
        uint32_t pos = (uint32_t)mixer_ch_get_pos(ch);
        // A loop wrapped (or the channel restarted): count from 0.
        trino_audio_streamed += pos >= trino_ch_pos[ch] ? pos - trino_ch_pos[ch] : pos;
        trino_ch_pos[ch] = pos;
    }
    trino_audio_ticks += TICKS_READ() - t0;
}

// out[0..10]: resident bytes, stream bytes, underruns, streamed frames,
// µs spent in trino_audio_poll, polls, µs the mixer waited for the RSP
// (mixing; libdragon's __mixer_profile_rsp), output buffers mixed, output
// samples per buffer, output rate.
void trino_audio_stats(uint32_t* out)
{
    out[0] = trino_audio_resident;
    out[1] = trino_audio_stream_bytes;
    out[2] = trino_audio_underruns;
    out[3] = trino_audio_streamed;
    out[4] = (uint32_t)TICKS_TO_US(trino_audio_ticks);
    out[5] = trino_audio_polls;
    out[6] = (uint32_t)TICKS_TO_US(__mixer_profile_rsp);
    out[7] = trino_audio_buffers;
    out[8] = (uint32_t)audio_get_buffer_length();
    out[9] = (uint32_t)audio_get_frequency();
}

// ---------------------------------------------------------------------------
// Assets

// DFS existence check (asset_load asserts on missing files, so callers must
// probe first). Takes a DFS-relative path like "/index.tsv".
int32_t trino_file_exists(const char* dfs_relative_path)
{
    int fd = dfs_open(dfs_relative_path);
    if (fd < 0) return 0;
    dfs_close(fd);
    return 1;
}

// Loads a whole DFS file into a malloc'd buffer. Caller frees with free().
void* trino_asset_load(const char* dfs_path, uint32_t* size_out)
{
    int size = 0;
    void* buf = asset_load(dfs_path, &size);
    if (size_out) *size_out = (uint32_t)size;
    return buf;
}

void trino_free(void* ptr)
{
    free(ptr);
}

// ---------------------------------------------------------------------------
// Save data: the cartridge EEPROM (4 Kbit = 512 B, 16 Kbit = 2 KB). The ROM
// header declares it (ed64romconfig --savetype eeprom16k) so emulators and
// flashcarts attach one. Framing/checksums live on the Rust side.

// Bytes of EEPROM present (0 = none detected).
uint32_t trino_save_capacity(void)
{
    if (eeprom_present() == EEPROM_NONE) return 0;
    return (uint32_t)(eeprom_total_blocks() * EEPROM_BLOCK_SIZE);
}

static __attribute__((noinline)) void trino_save_read_body(uint8_t* dest, uint32_t start, uint32_t len)
{
    eeprom_read_bytes(dest, start, len);
}

static __attribute__((noinline)) void trino_save_write_body(const uint8_t* src, uint32_t start, uint32_t len)
{
    eeprom_write_bytes(src, start, len);
}

// ---------------------------------------------------------------------------
// Stack watermark (diagnostics): libdragon's main stack is a fixed 64 KB at
// the top of RDRAM. trino_stack_paint() fills the unused part with a
// pattern at boot; trino_stack_used() reports the deepest use since.

#define TRINO_STACK_SIZE (64 * 1024)
#define TRINO_STACK_MAGIC 0x5EEDF00Du
static uint32_t* trino_stack_low = NULL;
static uint32_t trino_stack_top = 0;

void trino_stack_paint(void)
{
    uint32_t sp;
    __asm__ volatile("move %0, $sp" : "=r"(sp));
    // The stack occupies [top - 64K, top); `top` is above the current sp.
    trino_stack_top = (sp + 0xFFFF) & ~0xFFFFu;
    trino_stack_low = (uint32_t*)(trino_stack_top - TRINO_STACK_SIZE + 256);
    for (uint32_t* p = trino_stack_low; (uint32_t)p < sp - 256; p++) {
        *p = TRINO_STACK_MAGIC;
    }
}

uint32_t trino_stack_used(void)
{
    if (!trino_stack_low) return 0;
    uint32_t* p = trino_stack_low;
    while ((uint32_t)p < trino_stack_top && *p == TRINO_STACK_MAGIC) p++;
    return trino_stack_top - (uint32_t)p;
}

// ---------------------------------------------------------------------------
// Entry: libdragon's crt calls main(); Rust takes over immediately.

void trino_rust_main(void);

int main(void)
{
    trino_stack_paint();
    trino_rust_main();
    return 0;
}

// Test hook: print the last framebuffer (RGBA5551) over ISViewer for the
// emulator golden image (`cargo xtask test n64` rebuilds the PNG):
// "TRINO_FBR:<y>:<x0>:<hex>" rows of 64 pixels, then
// "TRINO_FB_END:<w>x<h>:<fnv1a32 of the pixels>".
void trino_debug_fb_dump(void)
{
    if (!trino_last_fb) return;
    rspq_wait();
    const uint8_t* base = UncachedAddr(trino_last_fb->buffer);
    uint32_t w = trino_last_fb->width, h = trino_last_fb->height;
    uint32_t hash = 2166136261u;
    static char line[64 * 4 + 1];
    static const char hex[] = "0123456789abcdef";
    for (uint32_t y = 0; y < h; y++) {
        const uint16_t* row = (const uint16_t*)(base + y * trino_last_fb->stride);
        for (uint32_t x0 = 0; x0 < w; x0 += 64) {
            uint32_t n = (w - x0 < 64) ? (w - x0) : 64;
            for (uint32_t i = 0; i < n; i++) {
                uint16_t px = row[x0 + i];
                hash = (hash ^ (px >> 8)) * 16777619u;
                hash = (hash ^ (px & 0xFF)) * 16777619u;
                line[i * 4 + 0] = hex[(px >> 12) & 15];
                line[i * 4 + 1] = hex[(px >> 8) & 15];
                line[i * 4 + 2] = hex[(px >> 4) & 15];
                line[i * 4 + 3] = hex[px & 15];
            }
            line[n * 4] = 0;
            trino_progress++;
            debugf("TRINO_FBR:%lu:%lu:%s\n", (unsigned long)y, (unsigned long)x0, line);
        }
    }
    debugf("TRINO_FB_END:%lux%lu:%08lx\n", (unsigned long)w, (unsigned long)h,
           (unsigned long)hash);
}

// The same overlay assembled with TRINO_COLORED: per-channel light terms
// and an emissive add. Its code, data and functions are last in this file,
// so adding them moved no other shim function or variable (an N64 frame's
// time moves with code and data layout).
DEFINE_RSP_UCODE(rsp_trino_color);
static uint32_t trino_rsp_color_id;

static void trino_register_color_overlay(void)
{
    trino_rsp_color_id = rspq_overlay_register(&rsp_trino_color);
}

// The same draw through the colored overlay (rsp_trino_color's
// TrinoCmd_Draw): `w` as trino_rsp_draw's, then the 13 words of `c`.
#define TRINO_RSP_COLOR_WORDS 13
void trino_rsp_draw_colored(const uint32_t* w, const uint32_t* c)
{
    if (!trino_mode_3d) trino_3d_begin();
    rspq_write_t q = rspq_write_begin(trino_rsp_color_id, 0x1, TRINO_RSP_DRAW_WORDS + TRINO_RSP_COLOR_WORDS);
    rspq_write_arg(&q, w[0] & 0xFF);
    for (int i = 1; i < TRINO_RSP_DRAW_WORDS; i++) rspq_write_arg(&q, w[i]);
    for (int i = 0; i < TRINO_RSP_COLOR_WORDS; i++) rspq_write_arg(&q, c[i]);
    rspq_write_end(&q);
    trino_rsp_pending = 1;
}

// The overlay assembled with TRINO_COLORED and TRINO_TEXTURED: textured
// draws (S, T, W and 1/W per vertex). Last in this file, like the colored
// one, so no other shim symbol moves.
DEFINE_RSP_UCODE(rsp_trino_tex);
static uint32_t trino_rsp_tex_id;

// A textured draw through rsp_trino_tex's TrinoCmd_Draw, after
// trino_3d_texture_ex bound its texture and mode: `w` as trino_rsp_draw's,
// then the 13 color words and the 4 texture words of `c`.
#define TRINO_RSP_TEX_WORDS 17
static void trino_register_tex_overlay(void)
{
    trino_rsp_tex_id = rspq_overlay_register(&rsp_trino_tex);
}

void trino_rsp_draw_textured(const uint32_t* w, const uint32_t* c)
{
    rspq_write_t q = rspq_write_begin(trino_rsp_tex_id, 0x1, TRINO_RSP_DRAW_WORDS + TRINO_RSP_TEX_WORDS);
    rspq_write_arg(&q, w[0] & 0xFF);
    for (int i = 1; i < TRINO_RSP_DRAW_WORDS; i++) rspq_write_arg(&q, w[i]);
    for (int i = 0; i < TRINO_RSP_TEX_WORDS; i++) rspq_write_arg(&q, c[i]);
    rspq_write_end(&q);
    trino_rsp_pending = 1;
    trino_rsp_tex_pending = 1;
}

// The overlay assembled with TRINO_WATER as well: water surfaces displaced,
// lit, Fresnel-shaded and fogged on the RSP (ADR 0015, issue #69).
DEFINE_RSP_UCODE(rsp_trino_water);
static uint32_t trino_rsp_water_id;

static void trino_register_water_overlay(void)
{
    trino_rsp_water_id = rspq_overlay_register(&rsp_trino_water);
}

// A water draw through rsp_trino_water's TrinoCmd_Draw, after
// trino_3d_water bound its texture and blend: `w` as trino_rsp_draw's, then
// the 17 color and texture words and the water block and ripple buffer
// addresses of `c`.
#define TRINO_RSP_WATER_WORDS 19
void trino_rsp_draw_water(const uint32_t* w, const uint32_t* c)
{
    rspq_write_t q = rspq_write_begin(trino_rsp_water_id, 0x1, TRINO_RSP_DRAW_WORDS + TRINO_RSP_WATER_WORDS);
    rspq_write_arg(&q, w[0] & 0xFF);
    for (int i = 1; i < TRINO_RSP_DRAW_WORDS; i++) rspq_write_arg(&q, w[i]);
    for (int i = 0; i < TRINO_RSP_WATER_WORDS; i++) rspq_write_arg(&q, c[i]);
    rspq_write_end(&q);
    trino_rsp_pending = 1;
    trino_rsp_tex_pending = 1;
}

// The o32/o64 seam (ADR 0002): Rust calls these with o32's 16-byte
// argument save area, but gcc compiles them for o64, whose callee may spill
// a2/a3 into the caller's 16(sp)..31(sp). That is the Rust caller's own
// frame: trino_3d_texture_ex spilling `flags` there overwrote a pointer
// submit_textured had saved at 16(sp), and the next call read it back as
// garbage (an address error in ares, 2026-10-18). Every export with three
// or four arguments is therefore this stub: its own frame holds the 32-byte
// save area the real function (`*_body`) may spill into, and the call is
// not a tail call (a jump would hand the body the Rust frame again).
#define TRINO_NOT_A_TAIL_CALL() __asm__ volatile("" ::: "memory")
void trino_fill_rect(uint32_t xy, uint32_t wh, uint32_t color)
{
    trino_fill_rect_body(xy, wh, color);
    TRINO_NOT_A_TAIL_CALL();
}

void trino_text_run(const uint32_t* g, uint32_t n, uint32_t scale)
{
    trino_text_run_body(g, n, scale);
    TRINO_NOT_A_TAIL_CALL();
}

void trino_tri(const float* pts, const uint8_t* colors, const float* zs)
{
    trino_tri_body(pts, colors, zs);
    TRINO_NOT_A_TAIL_CALL();
}

uint32_t trino_3d_texture_ex(void* sprite, void* detail, uint32_t flags)
{
    uint32_t r = trino_3d_texture_ex_body(sprite, detail, flags);
    TRINO_NOT_A_TAIL_CALL();
    return r;
}

uint32_t trino_3d_water(void* sprite, void* detail, uint32_t flags, uint32_t scroll)
{
    uint32_t r = trino_3d_water_body(sprite, detail, flags, scroll);
    TRINO_NOT_A_TAIL_CALL();
    return r;
}

void trino_wav_play(void* wav, uint32_t channel, uint32_t looped)
{
    trino_wav_play_body(wav, channel, looped);
    TRINO_NOT_A_TAIL_CALL();
}

void trino_ch_volume(uint32_t channel, uint32_t lvol_q16, uint32_t rvol_q16)
{
    trino_ch_volume_body(channel, lvol_q16, rvol_q16);
    TRINO_NOT_A_TAIL_CALL();
}

void trino_save_read(uint8_t* dest, uint32_t start, uint32_t len)
{
    trino_save_read_body(dest, start, len);
    TRINO_NOT_A_TAIL_CALL();
}

void trino_save_write(const uint8_t* src, uint32_t start, uint32_t len)
{
    trino_save_write_body(src, start, len);
    TRINO_NOT_A_TAIL_CALL();
}

// Write `len` bytes at cached address `p` back to RDRAM before the RSP
// DMAs them (the water draws' per-frame data: cached stores and one
// writeback cost less than a word at a time to uncached memory).
// Wake the RSP for what was queued (streaming, see ffi.rs): rspq only
// starts an idle RSP on a flush.
void trino_rsp_flush(void)
{
    rspq_flush();
}

void trino_cache_writeback(const void* p, uint32_t len)
{
    data_cache_hit_writeback(p, len);
}

// Claim the 16-byte lines at cached `p` (16-aligned, `len` a multiple of 16)
// for writing without reading them from RDRAM first (Create Dirty
// Exclusive): the caller overwrites every byte the RSP will read.
void trino_cache_create(void* p, uint32_t len)
{
    for (char* a = (char*)p; a < (char*)p + len; a += 16)
        __asm__ volatile("cache 0x0D, 0(%0)" :: "r"(a));
}

// A palette draw (GitHub #122) through rsp_trino_color's or rsp_trino_tex's
// TrinoCmd_DrawPal: `w` as trino_rsp_draw's (its word 0 says textured,
// rsp::TEXTURED, and its word 3 is the overlay's palette flag), then the 13
// color words (or 17 color and texture words) of `c`, then the 10 words of
// the palette table `p` (rsp::palette_entry). Three arguments: the o32/o64
// seam above, so the work is in `*_body`. Last in this file, like the
// overlays' other entries, so no other shim symbol moves.
#define TRINO_RSP_PAL_WORDS 10
static __attribute__((noinline)) void trino_rsp_draw_palette_body(const uint32_t* w, const uint32_t* c, const uint32_t* p)
{
    int tex = (w[0] & 0x200) != 0;
    int n = tex ? TRINO_RSP_TEX_WORDS : TRINO_RSP_COLOR_WORDS;
    if (!tex && !trino_mode_3d) trino_3d_begin();
    rspq_write_t q = rspq_write_begin(tex ? trino_rsp_tex_id : trino_rsp_color_id, 0x2,
                                      TRINO_RSP_DRAW_WORDS + n + TRINO_RSP_PAL_WORDS);
    rspq_write_arg(&q, w[0] & 0xFF);
    for (int i = 1; i < TRINO_RSP_DRAW_WORDS; i++) rspq_write_arg(&q, w[i]);
    for (int i = 0; i < n; i++) rspq_write_arg(&q, c[i]);
    for (int i = 0; i < TRINO_RSP_PAL_WORDS; i++) rspq_write_arg(&q, p[i]);
    rspq_write_end(&q);
    trino_rsp_pending = 1;
    if (tex) trino_rsp_tex_pending = 1;
}

void trino_rsp_draw_palette(const uint32_t* w, const uint32_t* c, const uint32_t* p)
{
    trino_rsp_draw_palette_body(w, c, p);
    TRINO_NOT_A_TAIL_CALL();
}
