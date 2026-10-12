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
// Textured 3D (citro3d): citro2d draws untextured triangles fine but cannot
// map UVs, so textured triangles take a tiny citro3d path of their own:
// our shader (tri3d.v.pica), immediate-mode vertices, one texture per batch,
// TEXTURE0 * vertex color. C2D_Flush before, C2D_Prepare after (citro2d's
// documented way to share the GPU).

static DVLB_s* s_tri3d_dvlb = NULL;
static shaderProgram_s s_tri3d;
static int s_tri3d_proj = -1;
static C3D_Mtx s_proj_top;
static float s_uv_l, s_uv_t, s_uv_r, s_uv_b;

static void trino_tex3d_init(void)
{
    s_tri3d_dvlb = DVLB_ParseFile((u32*)tri3d_shbin, tri3d_shbin_size);
    shaderProgramInit(&s_tri3d);
    shaderProgramSetVsh(&s_tri3d, &s_tri3d_dvlb->DVLE[0]);
    s_tri3d_proj = shaderInstanceGetUniformLocation(s_tri3d.vertexShader, "projection");
    // Same projection citro2d builds for the top screen.
    Mtx_OrthoTilt(&s_proj_top, 0.0f, 400.0f, 240.0f, 0.0f, 1.0f, -1.0f, true);
}

// ---------------------------------------------------------------------------
// Boot + logging

static C3D_RenderTarget* s_top = NULL;

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
    C2D_SceneBegin(s_top);
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

// ---------------------------------------------------------------------------
// 3D triangles: the engine transforms and lights on the CPU
// (trino_core::render3d); citro2d only rasterizes gouraud-shaded tris.

// Symmetry with the N64 shim (no mode switch needed on citro2d).
void trino_3d_begin(void)
{
}

// pts: 6 floats (x0,y0,x1,y1,x2,y2) in screen pixels;
// colors: 12 bytes (r,g,b,a per vertex);
// depth: citro2d z coordinate (larger = nearer; depth-tested by citro3d).
void trino_tri(const float* pts, const uint8_t* c, float depth)
{
    C2D_DrawTriangle(pts[0], pts[1], C2D_Color32(c[0], c[1], c[2], c[3]),
                     pts[2], pts[3], C2D_Color32(c[4], c[5], c[6], c[7]),
                     pts[4], pts[5], C2D_Color32(c[8], c[9], c[10], c[11]), depth);
}

// ---------------------------------------------------------------------------
// Save data: one file on the SD card, replaced atomically (write a temp
// file, then rename). Framing/checksums live on the Rust side.

#define TRINO_SAVE_DIR "sdmc:/3ds/trino"
#define TRINO_SAVE_PATH TRINO_SAVE_DIR "/save.bin"
#define TRINO_SAVE_TMP TRINO_SAVE_DIR "/save.tmp"

// Reads up to `cap` bytes; returns the file size, 0 if there is no save,
// -1 on error.
int32_t trino_save_load(uint8_t* dest, uint32_t cap)
{
    FILE* f = fopen(TRINO_SAVE_PATH, "rb");
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

// Start a textured batch: bind `sheet`'s image as a wrapping texture.
void trino_3d_tex_begin(void* sheet)
{
    C2D_Flush();
    C3D_BindProgram(&s_tri3d);
    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, s_tri3d_proj, &s_proj_top);
    C3D_AttrInfo* attr = C3D_GetAttrInfo();
    AttrInfo_Init(attr);
    AttrInfo_AddLoader(attr, 0, GPU_FLOAT, 4); // x, y, depth, w
    AttrInfo_AddLoader(attr, 1, GPU_FLOAT, 2); // u, v
    AttrInfo_AddLoader(attr, 2, GPU_FLOAT, 4); // r, g, b, a
    C3D_CullFace(GPU_CULL_NONE); // the engine already culled
    C2D_Image img = C2D_SpriteSheetGetImage((C2D_SpriteSheet)sheet, 0);
    C3D_TexSetFilter(img.tex, GPU_LINEAR, GPU_LINEAR);
    C3D_TexSetWrap(img.tex, GPU_REPEAT, GPU_REPEAT);
    C3D_TexBind(0, img.tex);
    // UVs map through the image's sub-texture (tex3ds stores rows flipped
    // and pads to power-of-two; POT images span the whole texture).
    s_uv_l = img.subtex->left;
    s_uv_t = img.subtex->top;
    s_uv_r = img.subtex->right;
    s_uv_b = img.subtex->bottom;
    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    C3D_TexEnvSrc(env, C3D_Both, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
    C3D_TexEnvFunc(env, C3D_Both, GPU_MODULATE);
    for (int i = 1; i < 6; i++) {
        C3D_TexEnvInit(C3D_GetTexEnv(i));
    }
    C3D_ImmDrawBegin(GPU_TRIANGLES);
}

// 30 floats: per vertex x, y, depth (citro2d z), u, v, w, r, g, b, a.
void trino_tri_tex(const float* v)
{
    for (int i = 0; i < 3; i++) {
        const float* s = v + i * 10;
        C3D_ImmSendAttrib(s[0], s[1], s[2], s[5]);
        C3D_ImmSendAttrib(s_uv_l + s[3] * (s_uv_r - s_uv_l),
                          s_uv_t + s[4] * (s_uv_b - s_uv_t), 0.0f, 0.0f);
        C3D_ImmSendAttrib(s[6], s[7], s[8], s[9]);
    }
}

// End the textured batch and hand the GPU back to citro2d.
void trino_3d_tex_end(void)
{
    C3D_ImmDrawEnd();
    C2D_Prepare();
}

// Test hook: RGBA8888 of the top screen at (x, y) (landscape, y down) from
// the LCD back buffer; 0 if unavailable. Waits two vblanks so the frame's
// display transfer has landed; callers draw the probed frame several times
// so both LCD buffers hold it. The LCD buffer is column-major, bottom-up.
uint32_t trino_debug_pixel(uint32_t x, uint32_t y)
{
    if (x >= 400 || y >= 240) return 0;
    gspWaitForVBlank();
    gspWaitForVBlank();
    u16 w = 0, h = 0;
    const u8* fb = gfxGetFramebuffer(GFX_TOP, GFX_LEFT, &w, &h);
    if (!fb || w != 240) return 0;
    GSPGPU_FramebufferFormat fmt = gfxGetScreenFormat(GFX_TOP);
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
