// Trino's C shim over libctru + citro2d/citro3d.
//
// WHY A SHIM (here, unlike the N64 there is no ABI hazard — both sides are
// ARM EABI-hf): citro2d is mostly static-inline C, which Rust cannot call,
// and keeping the exact same shim surface as the N64 backend
// (crates/platform-n64/shim) makes the platform crates near-identical. We
// keep the same discipline anyway:
//
//   - max 4 arguments, all of them i32/u32/f32 or pointers
//   - no by-value structs (pass pointers to #[repr(C)] structs instead)
//   - no variadics (format on the Rust side, pass the final string)
//   - returns are void, i32/u32, f32 or a pointer
//
// Compiled by `cargo xtask build 3ds` with devkitARM's arm-none-eabi-gcc
// against the installed libctru headers, so API drift in libctru/citro2d is
// a compile error here — never silent UB. Rust declarations live in
// ../src/ffi.rs and MUST match.

#include <stdint.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/stat.h>
#include <3ds.h>
#include <citro2d.h>

// tri3d.v.pica compiled by picasso and embedded as a C array by
// `cargo xtask build 3ds` (target/3ds/tri3d_shbin.h).
#include "tri3d_shbin.h"

// ---------------------------------------------------------------------------
// 3D triangles (citro3d): the engine's software T&L projects and lights
// every vertex; the GPU only rasterizes. All 3D goes through one citro3d
// path — our shader (tri3d.v.pica), a per-frame vertex buffer in linear
// memory, one draw per texture run — with a depth value PER VERTEX, so the
// depth test resolves overlap per pixel like the PC/N64 z-buffers.
// (citro2d's C2D_DrawTriangle takes one depth for the whole triangle; big
// floor triangles then painted over whatever stood on them — the
// castle64 bug the `occlusion` self-test step reproduces.)
static DVLB_s* s_tri3d_dvlb = NULL;
static shaderProgram_s s_tri3d;
static int s_tri3d_proj = -1;
static int s_tri3d_detail = -1;
// The next 3D draws' sampling (trino_3d_material): point filter, and a
// detail sheet multiplied over the texture (texture unit 1) tiling
// s_detail_scale times per base repeat.
static int s_point = 0;
static C2D_SpriteSheet s_detail_sheet = NULL;
static float s_detail_scale = 1.0f;
// One projection per screen (citro2d's own for each): 3D drawn after
// trino_set_screen(1) lands on the touch screen, projected to its 320x240.
static C3D_Mtx s_proj_top;
static C3D_Mtx s_proj_bottom;
static const C3D_Mtx* s_proj = &s_proj_top;

// Per-frame vertex buffer: x, y, depth, w | u, v | r, g, b, a (10 floats).
// C3D_FRAME_SYNCDRAW in trino_frame_begin waits for the previous frame's
// GPU work, so the buffer restarts at 0 every frame.
#define TRI3D_FLOATS 10
#define TRI3D_MAX_VERTS (3 * 16384)
static float* s_vbuf = NULL;
static uint32_t s_vused = 0;
static int s_vfull_warned = 0;

static void trino_tex3d_init(void)
{
    s_tri3d_dvlb = DVLB_ParseFile((u32*)tri3d_shbin, tri3d_shbin_size);
    shaderProgramInit(&s_tri3d);
    shaderProgramSetVsh(&s_tri3d, &s_tri3d_dvlb->DVLE[0]);
    s_tri3d_proj = shaderInstanceGetUniformLocation(s_tri3d.vertexShader, "projection");
    s_tri3d_detail = shaderInstanceGetUniformLocation(s_tri3d.vertexShader, "detail");
    // The same projections citro2d builds for each screen.
    Mtx_OrthoTilt(&s_proj_top, 0.0f, 400.0f, 240.0f, 0.0f, 1.0f, -1.0f, true);
    Mtx_OrthoTilt(&s_proj_bottom, 0.0f, 320.0f, 240.0f, 0.0f, 1.0f, -1.0f, true);
    s_vbuf = (float*)linearAlloc(TRI3D_MAX_VERTS * TRI3D_FLOATS * sizeof(float));
}

// ---------------------------------------------------------------------------
// Boot + logging

static C3D_RenderTarget* s_top = NULL;
// The touch screen (320x240), 2D and 3D (trino_set_screen).
static C3D_RenderTarget* s_bottom = NULL;

// Main-thread stack. libctru's weak default is 32 KB; game states live in
// one struct (fixed-capacity stores like EntityStore, no allocator in game
// code) and console self-tests run two games, so give Rust real headroom.
u32 __stacksize__ = 256 * 1024;

void trino_init(void)
{
    gfxInitDefault();
    romfsInit();
    C3D_Init(C3D_DEFAULT_CMDBUF_SIZE);
    C2D_Init(C2D_DEFAULT_MAX_OBJECTS);
    C2D_Prepare();
    // Trino tints are multiplicative (texel * tint), like the PC and N64
    // backends.
    C2D_SetTintMode(C2D_TintMult);
    s_top = C2D_CreateScreenTarget(GFX_TOP, GFX_LEFT);
    s_bottom = C2D_CreateScreenTarget(GFX_BOTTOM, GFX_LEFT);
    ndspInit();
    trino_tex3d_init();
}

// svcOutputDebugString: shows up in Azahar/Citra logs and GDB.
void trino_log(const char* msg)
{
    svcOutputDebugString(msg, strlen(msg));
}

// Rust panic lands here: report and halt (responding to HOME so the
// emulator/console can still close the app).
void trino_panic(const char* msg)
{
    svcOutputDebugString("TRINO_PANIC: ", 13);
    svcOutputDebugString(msg, strlen(msg));
    while (aptMainLoop()) {
        gspWaitForVBlank();
    }
    exit(1);
}

uint32_t trino_ticks_us(void)
{
    // SYSCLOCK_ARM11 = 268111856 Hz -> ~268.1 ticks per microsecond.
    return (uint32_t)(svcGetSystemTick() / (SYSCLOCK_ARM11 / 1000000));
}

// The 3DS main loop must stop when the applet manager says so (HOME button,
// power). The Rust loop polls this once per frame.
int32_t trino_app_running(void)
{
    return aptMainLoop() ? 1 : 0;
}

// ---------------------------------------------------------------------------
// Frame

void trino_frame_begin(uint32_t rgba8888)
{
    C3D_FrameBegin(C3D_FRAME_SYNCDRAW);
    C2D_TargetClear(s_top, C2D_Color32((rgba8888 >> 24) & 0xFF, (rgba8888 >> 16) & 0xFF,
                                       (rgba8888 >> 8) & 0xFF, 0xFF));
    // The touch screen's own background (games draw their UI on it).
    C2D_TargetClear(s_bottom, C2D_Color32(16, 18, 28, 0xFF));
    C2D_SceneBegin(s_top);
    s_proj = &s_proj_top;
    s_vused = 0;
}

// Later draws (2D and 3D) go to the touch screen (1) or the top screen (0),
// each with its own projection. The Rust side only ever moves 0 -> 1.
void trino_set_screen(uint32_t bottom)
{
    C2D_SceneBegin(bottom ? s_bottom : s_top);
    s_proj = bottom ? &s_proj_bottom : &s_proj_top;
}

void trino_frame_end(void)
{
    C3D_FrameEnd(0);
}

// ---------------------------------------------------------------------------
// Sprites — one .t3x per asset, image 0 of its sheet.

void* trino_sprite_load(const char* romfs_path)
{
    return C2D_SpriteSheetLoad(romfs_path);
}

uint32_t trino_sprite_size(void* sheet)
{
    C2D_Image img = C2D_SpriteSheetGetImage((C2D_SpriteSheet)sheet, 0);
    return ((uint32_t)img.subtex->width << 16) | (uint32_t)img.subtex->height;
}

// Layout mirrored by TrinoBlit in ../src/ffi.rs. Fields only, no bitfields.
typedef struct {
    float x, y;
    float scale_x, scale_y;
    float theta;          // radians
    uint32_t flip_x;      // 0/1
    uint32_t flip_y;      // 0/1
    uint32_t tint;        // RGBA8888; 0xFFFFFFFF = no tint
    float depth;          // citro2d z (larger = nearer; layers 2D vs 3D)
    int32_t src_x, src_y; // source sub-rect (sprite pixels)
    int32_t src_w, src_h; // 0 = whole sprite
} trino_blit_t;

void trino_sprite_blit(void* sheet, const trino_blit_t* p)
{
    C2D_Image img = C2D_SpriteSheetGetImage((C2D_SpriteSheet)sheet, 0);
    // Sub-rect (atlases, glyphs): a sub-texture whose UVs are interpolated
    // inside the image's own (works whatever way tex3ds oriented them).
    Tex3DS_SubTexture sub;
    if (p->src_w > 0 && p->src_h > 0) {
        const Tex3DS_SubTexture* full = img.subtex;
        float fw = (float)full->width, fh = (float)full->height;
        float du = full->right - full->left, dv = full->bottom - full->top;
        sub.width = (u16)p->src_w;
        sub.height = (u16)p->src_h;
        sub.left = full->left + du * ((float)p->src_x / fw);
        sub.right = full->left + du * ((float)(p->src_x + p->src_w) / fw);
        sub.top = full->top + dv * ((float)p->src_y / fh);
        sub.bottom = full->top + dv * ((float)(p->src_y + p->src_h) / fh);
        img.subtex = &sub;
    }
    float w = (float)img.subtex->width * p->scale_x;
    float h = (float)img.subtex->height * p->scale_y;

    C2D_DrawParams params = {
        // Negative extents flip the image; shift the origin so the sprite
        // covers the same rect either way (Trino positions are top-left).
        .pos = {
            p->flip_x ? p->x + w : p->x,
            p->flip_y ? p->y + h : p->y,
            p->flip_x ? -w : w,
            p->flip_y ? -h : h,
        },
        .center = { 0.0f, 0.0f }, // rotation around the top-left, like rdpq
        .depth = p->depth,
        .angle = p->theta,
    };

    if (p->tint != 0xFFFFFFFFu) {
        C2D_ImageTint tint;
        C2D_PlainImageTint(&tint,
                           C2D_Color32((p->tint >> 24) & 0xFF, (p->tint >> 16) & 0xFF,
                                       (p->tint >> 8) & 0xFF, p->tint & 0xFF),
                           1.0f);
        C2D_DrawImage(img, &params, &tint);
    } else {
        C2D_DrawImage(img, &params, NULL);
    }
}

// Solid rectangle (fill_rect): citro2d's own, no texture.
typedef struct {
    float x, y, w, h;
    float depth;
    uint32_t color; // RGBA8888
} trino_fill_t;

void trino_fill_rect(const trino_fill_t* f)
{
    uint32_t c = f->color;
    C2D_DrawRectSolid(f->x, f->y, f->depth, f->w, f->h,
                      C2D_Color32((c >> 24) & 0xFF, (c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF));
}

// ---------------------------------------------------------------------------
// 3D triangles: the engine transforms and lights on the CPU
// (trino_core::render3d); citro2d only rasterizes gouraud-shaded tris.

// Sampling of the next trino_3d_draw calls: `flags` bit 1 point filter,
// bits 4..7 the detail layer's repeats per base repeat (log2); `detail`
// (NULL = none) is a sprite sheet multiplied over the texture.
void trino_3d_material(void* detail, uint32_t flags)
{
    s_point = (flags >> 1) & 1;
    s_detail_sheet = (C2D_SpriteSheet)detail;
    s_detail_scale = (float)(1u << ((flags >> 4) & 15));
}

// Draw `count` vertices (count / 3 triangles), TRI3D_FLOATS each, in
// order: x, y (screen px), depth (citro2d z, larger = nearer), w (view
// depth: perspective-correct UVs; 1 = screen-linear), u, v (0..1 in the
// image), r, g, b, a (0..1). `sheet` = texture (image 0, tiled; clamp != 0:
// billboard addressing + half-alpha cutout), NULL = vertex colors only.
void trino_3d_draw(const float* v, uint32_t count, void* sheet, uint32_t clamp)
{
    if (!s_vbuf) return;
    if (s_vused + count > TRI3D_MAX_VERTS) {
        if (!s_vfull_warned) {
            s_vfull_warned = 1;
            trino_log("TRINO_WARN: 3D vertex buffer full, triangles dropped\n");
        }
        count = (TRI3D_MAX_VERTS - s_vused) / 3 * 3;
    }
    if (count == 0) return;

    float uv_l = 0.0f, uv_t = 0.0f, uv_r = 1.0f, uv_b = 1.0f;
    C2D_Image img = {0};
    if (sheet) {
        img = C2D_SpriteSheetGetImage((C2D_SpriteSheet)sheet, 0);
        // UVs map through the image's sub-texture (tex3ds stores rows
        // flipped and pads to power-of-two).
        uv_l = img.subtex->left;
        uv_t = img.subtex->top;
        uv_r = img.subtex->right;
        uv_b = img.subtex->bottom;
    }
    float* dst = s_vbuf + s_vused * TRI3D_FLOATS;
    for (uint32_t i = 0; i < count; i++) {
        const float* sv = v + i * TRI3D_FLOATS;
        float* dv = dst + i * TRI3D_FLOATS;
        dv[0] = sv[0];
        dv[1] = sv[1];
        dv[2] = sv[2];
        dv[3] = sv[3];
        dv[4] = uv_l + sv[4] * (uv_r - uv_l);
        dv[5] = uv_t + sv[5] * (uv_b - uv_t);
        dv[6] = sv[6];
        dv[7] = sv[7];
        dv[8] = sv[8];
        dv[9] = sv[9];
    }
    GSPGPU_FlushDataCache(dst, count * TRI3D_FLOATS * sizeof(float));

    C2D_Flush();
    C3D_BindProgram(&s_tri3d);
    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, s_tri3d_proj, s_proj);
    int detail = sheet && s_detail_sheet;
    C3D_FVUnifSet(GPU_VERTEX_SHADER, s_tri3d_detail, s_detail_scale, s_detail_scale, 0.0f, 0.0f);
    C3D_AttrInfo* attr = C3D_GetAttrInfo();
    AttrInfo_Init(attr);
    AttrInfo_AddLoader(attr, 0, GPU_FLOAT, 4); // x, y, depth, w
    AttrInfo_AddLoader(attr, 1, GPU_FLOAT, 2); // u, v
    AttrInfo_AddLoader(attr, 2, GPU_FLOAT, 4); // r, g, b, a
    C3D_BufInfo* buf = C3D_GetBufInfo();
    BufInfo_Init(buf);
    BufInfo_Add(buf, s_vbuf, TRI3D_FLOATS * sizeof(float), 3, 0x210);
    C3D_CullFace(GPU_CULL_NONE); // the engine already culled
    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    if (sheet) {
        GPU_TEXTURE_FILTER_PARAM filter = s_point ? GPU_NEAREST : GPU_LINEAR;
        C3D_TexSetFilter(img.tex, filter, filter);
        if (clamp) {
            C3D_TexSetWrap(img.tex, GPU_CLAMP_TO_EDGE, GPU_CLAMP_TO_EDGE);
            C3D_AlphaTest(true, GPU_GREATER, 127);
        } else {
            C3D_TexSetWrap(img.tex, GPU_REPEAT, GPU_REPEAT);
            C3D_AlphaTest(true, GPU_GREATER, 0);
        }
        C3D_TexBind(0, img.tex);
        C3D_TexEnvSrc(env, C3D_Both, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
        C3D_TexEnvFunc(env, C3D_Both, GPU_MODULATE);
    } else {
        C3D_AlphaTest(false, GPU_ALWAYS, 0);
        C3D_TexEnvSrc(env, C3D_Both, GPU_PRIMARY_COLOR, 0, 0);
        C3D_TexEnvFunc(env, C3D_Both, GPU_REPLACE);
    }
    for (int i = 1; i < 6; i++) {
        C3D_TexEnvInit(C3D_GetTexEnv(i));
    }
    if (detail) {
        // Stage 1: the lit texel times the detail texel (alpha stays).
        C2D_Image dimg = C2D_SpriteSheetGetImage(s_detail_sheet, 0);
        GPU_TEXTURE_FILTER_PARAM filter = s_point ? GPU_NEAREST : GPU_LINEAR;
        C3D_TexSetFilter(dimg.tex, filter, filter);
        C3D_TexSetWrap(dimg.tex, GPU_REPEAT, GPU_REPEAT);
        C3D_TexBind(1, dimg.tex);
        C3D_TexEnv* env1 = C3D_GetTexEnv(1);
        C3D_TexEnvSrc(env1, C3D_RGB, GPU_PREVIOUS, GPU_TEXTURE1, 0);
        C3D_TexEnvFunc(env1, C3D_RGB, GPU_MODULATE);
        C3D_TexEnvSrc(env1, C3D_Alpha, GPU_PREVIOUS, 0, 0);
        C3D_TexEnvFunc(env1, C3D_Alpha, GPU_REPLACE);
    }
    C3D_DrawArrays(GPU_TRIANGLES, s_vused, count);
    s_vused += count;
    // Hand the GPU back to citro2d (program, attributes, buffers, texenv).
    C2D_Prepare();
}

// ---------------------------------------------------------------------------
// Save data: one file on the SD card, replaced atomically (write a temp
// file, then rename). Framing/checksums live on the Rust side.

#define TRINO_SAVE_DIR "sdmc:/3ds/trino"
#define TRINO_SAVE_PATH TRINO_SAVE_DIR "/save.bin"
#define TRINO_SAVE_TMP TRINO_SAVE_DIR "/save.tmp"

// Reads up to `cap` bytes; returns the file size, 0 if there is no save,
// -1 on error. A power cut between store's remove() and rename() leaves
// only the (complete) temp file: read that. The Rust side checks the frame.
int32_t trino_save_load(uint8_t* dest, uint32_t cap)
{
    FILE* f = fopen(TRINO_SAVE_PATH, "rb");
    if (!f) f = fopen(TRINO_SAVE_TMP, "rb");
    if (!f) return 0;
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    size_t want = (size_t)size < cap ? (size_t)size : cap;
    size_t got = fread(dest, 1, want, f);
    fclose(f);
    if (size < 0 || got != want) return -1;
    return (int32_t)size;
}

// Returns 0 on success, -1 on error.
int32_t trino_save_store(const uint8_t* src, uint32_t len)
{
    mkdir("sdmc:/3ds", 0777);
    mkdir(TRINO_SAVE_DIR, 0777);
    FILE* f = fopen(TRINO_SAVE_TMP, "wb");
    if (!f) return -1;
    size_t put = fwrite(src, 1, len, f);
    if (fclose(f) != 0 || put != len) return -1;
    remove(TRINO_SAVE_PATH);
    return rename(TRINO_SAVE_TMP, TRINO_SAVE_PATH) == 0 ? 0 : -1;
}

// Test hook: RGBA8888 of the top screen (bottom = 0) or the touch screen
// (bottom = 1) at (x, y) (landscape, y down) from the LCD back buffer; 0 if
// unavailable. Waits two vblanks so the frame's display transfer has
// landed; callers draw the probed frame several times so both LCD buffers
// hold it. The LCD buffer is column-major, bottom-up.
uint32_t trino_debug_pixel(uint32_t x, uint32_t y, uint32_t bottom)
{
    if (x >= (bottom ? 320u : 400u) || y >= 240) return 0;
    gspWaitForVBlank();
    gspWaitForVBlank();
    u16 w = 0, h = 0;
    gfxScreen_t scr = bottom ? GFX_BOTTOM : GFX_TOP;
    const u8* fb = gfxGetFramebuffer(scr, GFX_LEFT, &w, &h);
    if (!fb || w != 240) return 0;
    GSPGPU_FramebufferFormat fmt = gfxGetScreenFormat(scr);
    u32 bpp = fmt == GSP_RGBA8_OES ? 4 : (fmt == GSP_BGR8_OES ? 3 : 0);
    if (!bpp) return 0;
    const u8* p = fb + (x * 240 + (239 - y)) * bpp;
    u32 r, g, b;
    if (bpp == 4) { // ABGR in memory
        r = p[3]; g = p[2]; b = p[1];
    } else {        // BGR
        r = p[2]; g = p[1]; b = p[0];
    }
    return (r << 24) | (g << 16) | (b << 8) | 0xFF;
}

// Test hook: print the top screen as "TRINO_FBR:<y>:<x0>:<hex>" rows of 64
// RGBA5551 pixels, then "TRINO_FB_END:400x240:<fnv1a32 of the pixels>" —
// the N64 dump format, so `cargo xtask test 3ds` rebuilds the image with the
// same code (5-bit channels: enough for a tolerance check). Same buffer
// rules as trino_debug_pixel.
void trino_debug_fb_dump(uint32_t bottom)
{
    gspWaitForVBlank();
    gspWaitForVBlank();
    u16 w = 0, h = 0;
    gfxScreen_t scr = bottom ? GFX_BOTTOM : GFX_TOP;
    const uint32_t width = bottom ? 320 : 400;
    const u8* fb = gfxGetFramebuffer(scr, GFX_LEFT, &w, &h);
    if (!fb || w != 240) return;
    GSPGPU_FramebufferFormat fmt = gfxGetScreenFormat(scr);
    u32 bpp = fmt == GSP_RGBA8_OES ? 4 : (fmt == GSP_BGR8_OES ? 3 : 0);
    if (!bpp) return;
    static const char hex[] = "0123456789abcdef";
    static char line[96 + 64 * 4];
    uint32_t hash = 2166136261u;
    for (uint32_t y = 0; y < 240; y++) {
        for (uint32_t x0 = 0; x0 < width; x0 += 64) {
            uint32_t n = (width - x0 < 64) ? (width - x0) : 64;
            int at = snprintf(line, 96, "TRINO_FBR:%lu:%lu:", (unsigned long)y,
                              (unsigned long)x0);
            for (uint32_t i = 0; i < n; i++) {
                const u8* p = fb + ((x0 + i) * 240 + (239 - y)) * bpp;
                u32 r, g, b;
                if (bpp == 4) { r = p[3]; g = p[2]; b = p[1]; }
                else          { r = p[2]; g = p[1]; b = p[0]; }
                uint16_t px = (uint16_t)(((r >> 3) << 11) | ((g >> 3) << 6) | ((b >> 3) << 1) | 1);
                hash = (hash ^ (px >> 8)) * 16777619u;
                hash = (hash ^ (px & 0xFF)) * 16777619u;
                line[at++] = hex[(px >> 12) & 15];
                line[at++] = hex[(px >> 8) & 15];
                line[at++] = hex[(px >> 4) & 15];
                line[at++] = hex[px & 15];
            }
            line[at++] = '\n';
            svcOutputDebugString(line, at);
        }
    }
    int at = snprintf(line, 96, "TRINO_FB_END:%lux240:%08lx\n", (unsigned long)width,
                      (unsigned long)hash);
    svcOutputDebugString(line, at);
}

// ---------------------------------------------------------------------------
// Input — bit positions match trino_core::input::Button discriminants.

uint32_t trino_joypad_buttons(void)
{
    hidScanInput();
    uint32_t k = hidKeysHeld();
    uint32_t out = 0;
    if (k & KEY_A) out |= 1u << 0;
    if (k & KEY_B) out |= 1u << 1;
    if (k & KEY_X) out |= 1u << 2;
    if (k & KEY_Y) out |= 1u << 3;
    if (k & KEY_L) out |= 1u << 4;
    if (k & KEY_R) out |= 1u << 5;
    if (k & KEY_START) out |= 1u << 6;
    if (k & KEY_SELECT) out |= 1u << 7;
    if (k & KEY_DUP) out |= 1u << 8;
    if (k & KEY_DDOWN) out |= 1u << 9;
    if (k & KEY_DLEFT) out |= 1u << 10;
    if (k & KEY_DRIGHT) out |= 1u << 11;
    return out;
}

// Packed circle pad: high 16 bits = x, low 16 = y, as signed 16-bit each.
uint32_t trino_joypad_stick(void)
{
    circlePosition pos;
    hidCircleRead(&pos);
    uint16_t x = (uint16_t)pos.dx;
    uint16_t y = (uint16_t)pos.dy;
    return ((uint32_t)x << 16) | y;
}

// Touch screen (bottom, 320x240): bit 31 = touching, bits 16..24 = y,
// bits 0..9 = x, in bottom-screen pixels. hidScanInput already ran in
// trino_joypad_buttons this frame.
uint32_t trino_touch(void)
{
    if (!(hidKeysHeld() & KEY_TOUCH)) return 0;
    touchPosition t;
    hidTouchRead(&t);
    return (1u << 31) | ((uint32_t)(t.py & 0xff) << 16) | (uint32_t)(t.px & 0x3ff);
}

// ---------------------------------------------------------------------------
// Audio — raw PCM16 through ndsp. Baked format (cargo xtask assets 3ds):
// 12-byte header (u32 LE sample_rate, u32 LE channels, u32 LE frame count)
// followed by interleaved PCM16 LE samples.

typedef struct {
    void* data;          // linearAlloc'd sample memory (ndsp requirement)
    uint32_t nsamples;   // frames
    uint32_t sample_rate;
    uint32_t channels;
} trino_wav_t;

#define TRINO_NDSP_CHANNELS 16
static ndspWaveBuf s_wavebufs[TRINO_NDSP_CHANNELS];

void* trino_wav_load(const char* romfs_path)
{
    FILE* f = fopen(romfs_path, "rb");
    if (!f) return NULL;
    uint32_t header[3];
    if (fread(header, sizeof(header), 1, f) != 1) {
        fclose(f);
        return NULL;
    }
    trino_wav_t* wav = malloc(sizeof(trino_wav_t));
    wav->sample_rate = header[0];
    wav->channels = header[1] ? header[1] : 1;
    wav->nsamples = header[2];
    size_t bytes = (size_t)wav->nsamples * wav->channels * 2;
    wav->data = linearAlloc(bytes);
    if (!wav->data || fread(wav->data, 1, bytes, f) != bytes) {
        if (wav->data) linearFree(wav->data);
        free(wav);
        fclose(f);
        return NULL;
    }
    DSP_FlushDataCache(wav->data, bytes);
    fclose(f);
    return wav;
}

void trino_wav_play(void* wav_ptr, uint32_t channel, uint32_t looped)
{
    trino_wav_t* wav = wav_ptr;
    if (channel >= TRINO_NDSP_CHANNELS) return;
    ndspChnReset(channel);
    ndspChnSetInterp(channel, NDSP_INTERP_LINEAR);
    ndspChnSetRate(channel, (float)wav->sample_rate);
    ndspChnSetFormat(channel,
                     wav->channels == 2 ? NDSP_FORMAT_STEREO_PCM16 : NDSP_FORMAT_MONO_PCM16);
    ndspWaveBuf* buf = &s_wavebufs[channel];
    memset(buf, 0, sizeof(*buf));
    buf->data_vaddr = wav->data;
    buf->nsamples = wav->nsamples;
    buf->looping = looped != 0;
    ndspChnWaveBufAdd(channel, buf);
}

void trino_channel_stop(uint32_t channel)
{
    if (channel >= TRINO_NDSP_CHANNELS) return;
    ndspChnWaveBufClear(channel);
    s_wavebufs[channel].status = NDSP_WBUF_DONE;
}

// Per-channel left/right volume (pan + attenuation, music ducking): the
// front-left / front-right entries of the ndsp mix.
// Fixed-point volumes (65536 = 1.0): same signature as the N64 shim, where
// scalar floats after an int are unsafe across the o32/o64 boundary.
void trino_ch_volume(uint32_t channel, uint32_t lvol_q16, uint32_t rvol_q16)
{
    if (channel >= TRINO_NDSP_CHANNELS) return;
    float mix[12] = {0};
    mix[0] = lvol_q16 / 65536.0f;
    mix[1] = rvol_q16 / 65536.0f;
    ndspChnSetMix(channel, mix);
}

// Playing = the DSP is on it, or our buffer is still queued (right after a
// play the DSP has not picked it up yet: ndspChnIsPlaying lags a frame).
int32_t trino_ch_playing(uint32_t channel)
{
    if (channel >= TRINO_NDSP_CHANNELS) return 0;
    u8 st = s_wavebufs[channel].status;
    return (ndspChnIsPlaying(channel) || st == NDSP_WBUF_QUEUED || st == NDSP_WBUF_PLAYING) ? 1 : 0;
}

void trino_mixer_set_vol(uint32_t vol_q16)
{
    ndspSetMasterVol(vol_q16 / 65536.0f);
}

// ndsp is callback-driven; nothing to pump. Kept so the platform crates and
// game loops stay identical across consoles.
void trino_audio_poll(void)
{
}

// ---------------------------------------------------------------------------
// Assets

// RomFS existence check. Takes a romfs-relative path like "/index.tsv".
int32_t trino_file_exists(const char* romfs_relative_path)
{
    char full[256];
    snprintf(full, sizeof(full), "romfs:%s", romfs_relative_path);
    FILE* f = fopen(full, "rb");
    if (!f) return 0;
    fclose(f);
    return 1;
}

// Loads a whole romfs file into a malloc'd buffer. Caller frees with
// trino_free.
void* trino_asset_load(const char* romfs_path, uint32_t* size_out)
{
    FILE* f = fopen(romfs_path, "rb");
    if (!f) return NULL;
    fseek(f, 0, SEEK_END);
    long size = ftell(f);
    fseek(f, 0, SEEK_SET);
    if (size < 0) {
        fclose(f);
        return NULL;
    }
    void* buf = malloc((size_t)size);
    if (buf && fread(buf, 1, (size_t)size, f) != (size_t)size) {
        free(buf);
        buf = NULL;
    }
    fclose(f);
    if (buf && size_out) *size_out = (uint32_t)size;
    return buf;
}

void trino_free(void* ptr)
{
    free(ptr);
}

// ---------------------------------------------------------------------------
// Entry: the 3dsx crt0 calls main(); Rust takes over immediately.

void trino_rust_main(void);

int main(void)
{
    trino_rust_main();
    // Rust returns when aptMainLoop() says stop: tear down in reverse.
    ndspExit();
    C2D_Fini();
    C3D_Fini();
    romfsExit();
    gfxExit();
    return 0;
}
