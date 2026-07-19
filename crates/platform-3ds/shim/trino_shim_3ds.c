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
#include <3ds.h>
#include <citro2d.h>

// ---------------------------------------------------------------------------
// Boot + logging

static C3D_RenderTarget* s_top = NULL;

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
} trino_blit_t;

void trino_sprite_blit(void* sheet, const trino_blit_t* p)
{
    C2D_Image img = C2D_SpriteSheetGetImage((C2D_SpriteSheet)sheet, 0);
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
