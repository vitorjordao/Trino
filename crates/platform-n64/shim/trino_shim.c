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
#include <string.h>
#include <libdragon.h>

// ---------------------------------------------------------------------------
// Boot + logging

static void trino_check_trifmt(void);

// VI interrupt counter: an independent clock for perf reports (60 per
// emulated second on NTSC), to cross-check the count-register timings.
static volatile uint32_t trino_vblanks = 0;
static void trino_on_vi(void) { trino_vblanks++; }

// Standard render mode + magic-square RGB dither into the 16-bit
// framebuffer (libdragon's standard mode disables dithering).
static void trino_mode_standard(void)
{
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
    register_VI_handler(trino_on_vi);
}

void trino_log(const char* msg)
{
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
    surface_t* fb = display_get();
    trino_last_fb = fb;
    if (!trino_zbuf_ready) {
        trino_zbuf = surface_alloc(FMT_RGBA16, display_get_width(), display_get_height());
        trino_zbuf_ready = 1;
    }
    rdpq_attach(fb, &trino_zbuf);
    rdpq_clear(RGBA32((rgba8888 >> 24) & 0xFF, (rgba8888 >> 16) & 0xFF,
                      (rgba8888 >> 8) & 0xFF, 255));
    rdpq_clear_z(0xFFFC);
}

void trino_frame_end(void)
{
    rdpq_detach_show();
}

// ---------------------------------------------------------------------------
// Sprites

void* trino_sprite_load(const char* dfs_path)
{
    return sprite_load(dfs_path);
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

void trino_sprite_blit(void* sprite, const trino_blit_t* p)
{
    trino_mode_standard();
    rdpq_mode_alphacompare(1);
    if (p->tint != 0xFFFFFFFFu) {
        rdpq_set_prim_color(RGBA32((p->tint >> 24) & 0xFF, (p->tint >> 16) & 0xFF,
                                   (p->tint >> 8) & 0xFF, p->tint & 0xFF));
        rdpq_mode_combiner(RDPQ_COMBINER1((TEX0, 0, PRIM, 0), (TEX0, 0, PRIM, 0)));
    }
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

// ---------------------------------------------------------------------------
// 3D triangles: the engine transforms and lights on the CPU
// (trino_core::render3d); the RDP only rasterizes gouraud-shaded tris.

// Call once before a batch of trino_tri (shade combiner + hardware z-test).
void trino_3d_begin(void)
{
    trino_mode_standard();
    rdpq_mode_combiner(RDPQ_COMBINER_SHADE);
    rdpq_mode_zbuf(true, true);
}

// pts: 6 floats (x0,y0,x1,y1,x2,y2) in screen pixels;
// colors: 12 bytes (r,g,b,a per vertex);
// zs: 3 floats, normalized depth 0..1 (0 = near plane).
// Vertex layout is driven by TRIFMT_ZBUF_SHADE's own offsets so a libdragon
// layout change is a behavior-preserving recompile, not silent corruption.
void trino_tri(const float* pts, const uint8_t* colors, const float* zs)
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
// Input — bit positions match trino_core::input::Button discriminants.

uint32_t trino_joypad_buttons(void)
{
    joypad_poll();
    joypad_buttons_t b = joypad_get_buttons_held(JOYPAD_PORT_1);
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

void* trino_wav_load(const char* dfs_path)
{
    wav64_t* wav = malloc(sizeof(wav64_t));
    wav64_open(wav, dfs_path);
    return wav;
}

// `looped` is accepted for cross-platform symmetry but ignored here: on the
// N64, looping is baked into the wav64 file (audioconv64 --wav-loop).
void trino_wav_play(void* wav, uint32_t channel, uint32_t looped)
{
    (void)looped;
    wav64_play((wav64_t*)wav, (int)channel);
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
void trino_ch_volume(uint32_t channel, uint32_t lvol_q16, uint32_t rvol_q16)
{
    mixer_ch_set_vol((int)channel, lvol_q16 / 65536.0f, rvol_q16 / 65536.0f);
}

int32_t trino_ch_playing(uint32_t channel)
{
    return mixer_ch_playing((int)channel) ? 1 : 0;
}

void trino_audio_poll(void)
{
    while (audio_can_write()) {
        int16_t* buf = audio_write_begin();
        mixer_poll(buf, audio_get_buffer_length());
        audio_write_end();
    }
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

void trino_save_read(uint8_t* dest, uint32_t start, uint32_t len)
{
    eeprom_read_bytes(dest, start, len);
}

void trino_save_write(const uint8_t* src, uint32_t start, uint32_t len)
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
            debugf("TRINO_FBR:%lu:%lu:%s\n", (unsigned long)y, (unsigned long)x0, line);
        }
    }
    debugf("TRINO_FB_END:%lux%lu:%08lx\n", (unsigned long)w, (unsigned long)h,
           (unsigned long)hash);
}

