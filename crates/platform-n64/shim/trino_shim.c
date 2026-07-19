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

void trino_init(void)
{
    debug_init_isviewer();
    debug_init_usblog();
    display_init(RESOLUTION_320x240, DEPTH_16_BPP, 2, GAMMA_NONE, FILTERS_RESAMPLE);
    rdpq_init();
    joypad_init();
    timer_init();
    dfs_init(DFS_DEFAULT_LOCATION);
    audio_init(44100, 4);
    mixer_init(16);
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
    return (uint32_t)(timer_ticks() / (TICKS_PER_SECOND / 1000000));
}

// ---------------------------------------------------------------------------
// Frame

// Hardware z-buffer for the 3D triangles (allocated on first frame).
static surface_t trino_zbuf;
static int trino_zbuf_ready = 0;

void trino_frame_begin(uint32_t rgba8888)
{
    surface_t* fb = display_get();
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
} trino_blit_t;

void trino_sprite_blit(void* sprite, const trino_blit_t* p)
{
    rdpq_set_mode_standard();
    rdpq_mode_alphacompare(1);
    if (p->tint != 0xFFFFFFFFu) {
        rdpq_set_prim_color(RGBA32((p->tint >> 24) & 0xFF, (p->tint >> 16) & 0xFF,
                                   (p->tint >> 8) & 0xFF, p->tint & 0xFF));
        rdpq_mode_combiner(RDPQ_COMBINER1((TEX0, 0, PRIM, 0), (TEX0, 0, PRIM, 0)));
    }
    rdpq_blitparms_t parms = {
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
    rdpq_set_mode_standard();
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
    float v[3][8];
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

void trino_mixer_set_vol(float vol)
{
    // Master volume: apply per-channel on play instead; global gain not
    // exposed uniformly. v1: no-op placeholder kept for ABI stability.
    (void)vol;
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
// Entry: libdragon's crt calls main(); Rust takes over immediately.

void trino_rust_main(void);

int main(void)
{
    trino_rust_main();
    return 0;
}
