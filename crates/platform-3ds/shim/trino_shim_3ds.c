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
#include <malloc.h>
#include <unistd.h>
#include <arpa/inet.h>
#include <3ds.h>
#include <citro2d.h>

// tri3d.v.pica compiled by picasso and embedded as a C array by
// `cargo xtask build 3ds` (target/3ds/tri3d_shbin.h).
#include "tri3d_shbin.h"
// tnl3d.v.pica: vertex T&L on the GPU for static meshes (trino_gpu_draw).
#include "tnl3d_shbin.h"
// water.v.pica: water surfaces displaced and shaded on the GPU
// (trino_water_draw, ADR 0015).
#include "water_shbin.h"
// Which winding the GPU T&L path culls as back faces (checked by the
// `gpu tnl` self-test step against the CPU path).
#ifndef TRINO_TNL_CULL
#define TRINO_TNL_CULL GPU_CULL_FRONT_CCW
#endif

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
static int s_tri3d_detailoff = -1;
// Water (trino_3d_material flags bit 8, ADR 0015): the two layers blended
// half and half times the vertex color, alpha from the vertex, depth
// tested but not written; the detail layer offset by s_detail_off (its
// own flow).
static int s_water = 0;
static float s_detail_off[2] = {0.0f, 0.0f};
// The next 3D draws' sampling (trino_3d_material): point filter, and a
// detail sheet multiplied over the texture (texture unit 1) tiling
// s_detail_scale times per base repeat.
static int s_point = 0;
static C2D_SpriteSheet s_detail_sheet = NULL;
static float s_detail_scale = 1.0f;
// Shadows (trino_3d_material flags bit 9, ADR 0025): the vertex color
// blended by its alpha, depth tested but not written, with bits 10..12 the
// stencil's part (trino_shadow_stencil) and bit 13 the blob texture's
// alpha (s_blob, trino_shadow_blob) times the vertex alpha.
static int s_shadow = 0;
static int s_stencil = 0;
static int s_blob_on = 0;
static C3D_Tex s_blob;
static int s_blob_ready = 0;
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

// GPU T&L (tnl3d.v.pica): static meshes uploaded once to linear memory,
// drawn with per-draw uniforms (trino_gpu_mesh / trino_gpu_draw).
static DVLB_s* s_tnl3d_dvlb = NULL;
static shaderProgram_s s_tnl3d;
#define TNL_UNIFORMS 8
static int s_tnl3d_unif[TNL_UNIFORMS];
// The GPU holds the T&L program's state (program, attributes, texenv):
// consecutive trino_gpu_draw calls only change uniforms and the vertex
// buffer. Any other draw (citro2d, trino_3d_draw) or frame step leaves it
// first (trino_tnl_leave), which hands the GPU back to citro2d.
static int s_tnl_on = 0;

// The water program and its uniforms (in water.v.pica's order; mvp and
// model are matrices, the rest one vector each).
static DVLB_s* s_water_dvlb = NULL;
static shaderProgram_s s_water_prog;
#define WATER_UNIFORMS 23
static int s_water_unif[WATER_UNIFORMS];

static void trino_tnl_leave(void)
{
    if (s_tnl_on) {
        s_tnl_on = 0;
        C3D_CullFace(GPU_CULL_NONE);
        C2D_Prepare();
    }
}

static void trino_tex3d_init(void)
{
    s_tri3d_dvlb = DVLB_ParseFile((u32*)tri3d_shbin, tri3d_shbin_size);
    shaderProgramInit(&s_tri3d);
    shaderProgramSetVsh(&s_tri3d, &s_tri3d_dvlb->DVLE[0]);
    s_tri3d_proj = shaderInstanceGetUniformLocation(s_tri3d.vertexShader, "projection");
    s_tri3d_detail = shaderInstanceGetUniformLocation(s_tri3d.vertexShader, "detail");
    s_tri3d_detailoff = shaderInstanceGetUniformLocation(s_tri3d.vertexShader, "detailoff");
    // The same projections citro2d builds for each screen.
    Mtx_OrthoTilt(&s_proj_top, 0.0f, 400.0f, 240.0f, 0.0f, 1.0f, -1.0f, true);
    Mtx_OrthoTilt(&s_proj_bottom, 0.0f, 320.0f, 240.0f, 0.0f, 1.0f, -1.0f, true);
    s_vbuf = (float*)linearAlloc(TRI3D_MAX_VERTS * TRI3D_FLOATS * sizeof(float));

    s_tnl3d_dvlb = DVLB_ParseFile((u32*)tnl3d_shbin, tnl3d_shbin_size);
    shaderProgramInit(&s_tnl3d);
    shaderProgramSetVsh(&s_tnl3d, &s_tnl3d_dvlb->DVLE[0]);
    const char* names[TNL_UNIFORMS] = {
        "mvp", "lightdir", "ambient", "diffuse", "tint", "emissive", "fogrow", "fogcolor"};
    for (int i = 0; i < TNL_UNIFORMS; i++) {
        s_tnl3d_unif[i] = shaderInstanceGetUniformLocation(s_tnl3d.vertexShader, names[i]);
    }

    s_water_dvlb = DVLB_ParseFile((u32*)water_shbin, water_shbin_size);
    shaderProgramInit(&s_water_prog);
    shaderProgramSetVsh(&s_water_prog, &s_water_dvlb->DVLE[0]);
    const char* wnames[WATER_UNIFORMS] = {
        "mvp",      "model",   "wdir",     "wk",    "wamp",   "rx",       "rz",     "rrad",
        "rstr",     "lightdir", "ambient", "diffuse", "tint", "emissive", "eye",    "sky",
        "glint",    "fogrow",  "fogcolor", "scroll", "detailoff", "sub0",  "detail"};
    for (int i = 0; i < WATER_UNIFORMS; i++) {
        s_water_unif[i] = shaderInstanceGetUniformLocation(s_water_prog.vertexShader, wnames[i]);
    }
}

// ---------------------------------------------------------------------------
// Boot + logging

static C3D_RenderTarget* s_top = NULL;
// The top screen's right eye (issue #63, ADR 0032): made on the first
// stereo frame, drawn only by stereo frames (citro3d transfers a target to
// the LCD only in the frames that used it).
static C3D_RenderTarget* s_top_right = NULL;
// gfxSet3D's state (trino_stereo_mode).
static int s_stereo_on = 0;
// The touch screen (320x240), 2D and 3D (trino_set_screen).
static C3D_RenderTarget* s_bottom = NULL;

// Main-thread stack. libctru's weak default is 32 KB; game states live in
// one struct (fixed-capacity stores like EntityStore, no allocator in game
// code) and console self-tests run two games, so give Rust real headroom.
u32 __stacksize__ = 256 * 1024;

// 3dslink (real hardware, `cargo xtask run 3ds --device network`): libctru
// sets __3dslink_host from the launch arguments 3dslink sends. Then stdout
// and stderr go to the host's `3dslink -s` server, and trino_log writes
// there as well. Emulators and normal launches leave it 0: nothing changes.
#define TRINO_SOC_ALIGN 0x1000
#define TRINO_SOC_SIZE 0x100000
static u32* s_soc_buf = NULL;
static int s_link_sock = -1;

static void trino_debug_str(const char* msg)
{
    svcOutputDebugString(msg, strlen(msg));
}

static void trino_link_exit(void)
{
    if (s_link_sock >= 0) {
        fflush(stdout);
        fflush(stderr);
        close(s_link_sock);
        s_link_sock = -1;
    }
    socExit();
    free(s_soc_buf);
    s_soc_buf = NULL;
}

static void trino_link_init(void)
{
    if (__3dslink_host.s_addr == 0) {
        return;
    }
    s_soc_buf = (u32*)memalign(TRINO_SOC_ALIGN, TRINO_SOC_SIZE);
    if (s_soc_buf == NULL) {
        trino_debug_str("TRINO_WARN: 3dslink: no memory for the socket buffer\n");
        return;
    }
    if (R_FAILED(socInit(s_soc_buf, TRINO_SOC_SIZE))) {
        trino_debug_str("TRINO_WARN: 3dslink: socInit failed\n");
        free(s_soc_buf);
        s_soc_buf = NULL;
        return;
    }
    s_link_sock = link3dsStdio();
    if (s_link_sock < 0) {
        // No server listening: give the service and its 1 MiB back.
        trino_debug_str("TRINO_WARN: 3dslink: no host server\n");
        socExit();
        free(s_soc_buf);
        s_soc_buf = NULL;
        return;
    }
    atexit(trino_link_exit);
}

// Also to the 3dslink host when connected.
static void trino_link_write(const char* msg, size_t len)
{
    if (s_link_sock >= 0) {
        fwrite(msg, 1, len, stdout);
        fflush(stdout);
    }
}

static void trino_audio_init(void);
static void trino_audio_exit(void);

// A screen's target: citro2d's (C2D_CreateScreenTarget, the same output to
// the LCD) with a 24-bit depth and an 8-bit stencil in place of its 16-bit
// depth: shadows sit a few depth steps over their receiver at any
// distance, and planar shadows draw once per pixel through the stencil
// (ADR 0025). C2D_TargetClear clears the stencil to 0 with the depth.
static C3D_RenderTarget* trino_screen_target(gfxScreen_t screen, int height)
{
    C3D_RenderTarget* t =
        C3D_RenderTargetCreate(GSP_SCREEN_WIDTH, height, GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
    if (t) {
        C3D_RenderTargetSetOutput(
            t, screen, GFX_LEFT,
            GX_TRANSFER_FLIP_VERT(0) | GX_TRANSFER_OUT_TILED(0) | GX_TRANSFER_RAW_COPY(0) |
                GX_TRANSFER_IN_FORMAT(GX_TRANSFER_FMT_RGBA8) |
                GX_TRANSFER_OUT_FORMAT(GX_TRANSFER_FMT_RGB8) |
                GX_TRANSFER_SCALING(GX_TRANSFER_SCALE_NO));
    }
    return t;
}

void trino_init(void)
{
    trino_link_init();
    gfxInitDefault();
    romfsInit();
    // 4x the default command buffer: every GPU T&L draw adds its uniforms
    // and a draw command (a busy scene has hundreds).
    C3D_Init(C3D_DEFAULT_CMDBUF_SIZE * 4);
    C2D_Init(C2D_DEFAULT_MAX_OBJECTS);
    C2D_Prepare();
    // Trino tints are multiplicative (texel * tint), like the PC and N64
    // backends.
    C2D_SetTintMode(C2D_TintMult);
    s_top = trino_screen_target(GFX_TOP, GSP_SCREEN_HEIGHT_TOP);
    s_bottom = trino_screen_target(GFX_BOTTOM, GSP_SCREEN_HEIGHT_BOTTOM);
    trino_audio_init();
    trino_tex3d_init();
}

// The hardware tier (issue #54, ADR 0020): is this a New 3DS / New 2DS XL,
// and if so (and `want_speedup`), turn on its 804 MHz clock and L2 cache.
// Returns bit 0 = New 3DS, bit 1 = speedup on, bits 8.. = the application
// memory region in MiB (decoded by src/hw.rs). Call after trino_init.
uint32_t trino_hw_probe(uint32_t want_speedup)
{
    bool is_new = false;
    APT_CheckNew3DS(&is_new);
    uint32_t speedup = 0;
    if (is_new && want_speedup) {
        osSetSpeedupEnable(true);
        speedup = 1;
    }
    uint32_t app_mib = (uint32_t)(osGetMemRegionSize(MEMREGION_APPLICATION) >> 20);
    // An old 2DS or a New 2DS XL has no stereo screen (issue #63).
    uint32_t flat = 0;
    if (R_SUCCEEDED(cfguInit())) {
        u8 model = 0;
        if (R_SUCCEEDED(CFGU_GetSystemModel(&model)) &&
            (model == CFG_MODEL_2DS || model == CFG_MODEL_N2DSXL)) {
            flat = 1;
        }
        cfguExit();
    }
    return (is_new ? 1u : 0u) | (speedup << 1) | (flat << 2) | (app_mib << 8);
}

// The system language (issue #59): CFG_Language (0 Japanese, 1 English, ...
// 11 traditional Chinese, mapped to tags by src/language.rs), or -1 when the
// config service can't be read. Opens and closes CFGU itself; call once.
int trino_system_language(void)
{
    if (R_FAILED(cfguInit())) {
        return -1;
    }
    u8 lang = 0;
    Result rc = CFGU_GetSystemLanguage(&lang);
    cfguExit();
    return R_FAILED(rc) ? -1 : (int)lang;
}

// ---------------------------------------------------------------------------
// The worker core (issue #54 Phase 3, ADR 0020): one thread on the second
// CPU core for game jobs (AI #56, a crowd's animation). The audio thread
// (#53) shares that core at a higher priority (trino_audio_init). The New
// 3DS's core 2 is the app's whole; on an old 3DS the app gets core 1 for 30%
// of its time (APT_SetAppCpuTimeLimit: the system keeps the rest). Jobs are
// a function and one word, queued in a ring; trino_worker_wait blocks until
// every queued job has run. Without a worker (no core granted) nothing is
// queued and the Rust side runs each job inline.

// The second core, one policy for every thread that wants it (the worker
// here, the audio thread #53): core 2 on a New 3DS, else core 1 once the app
// has asked for 30% of it (asked once; the answer is kept). Returns the
// thread and its core, or NULL and -1 when neither core was granted.
static Thread trino_second_core_thread(ThreadFunc fn, size_t stack, s32 prio, bool new3ds,
                                       int32_t* core)
{
    static int s_core1 = 0; // 0: not asked yet, 1: granted, -1: refused
    if (new3ds) {
        Thread t = threadCreate(fn, NULL, stack, prio, 2, false);
        if (t) {
            *core = 2;
            return t;
        }
    }
    if (s_core1 == 0) {
        s_core1 = R_SUCCEEDED(APT_SetAppCpuTimeLimit(30)) ? 1 : -1;
    }
    if (s_core1 > 0) {
        Thread t = threadCreate(fn, NULL, stack, prio, 1, false);
        if (t) {
            *core = 1;
            return t;
        }
    }
    *core = -1;
    return NULL;
}

#define TRINO_JOBS 32

typedef void (*trino_job_fn)(uint32_t);

static struct {
    trino_job_fn fn;
    uint32_t arg;
} s_jobs[TRINO_JOBS];
static uint32_t s_job_head; // next job to run (worker)
static uint32_t s_job_tail; // next free slot (main thread)
static LightLock s_job_lock;
static LightSemaphore s_job_ready;
static LightEvent s_job_idle;
static uint32_t s_job_pending;
static volatile bool s_worker_quit;
static Thread s_worker;
static int32_t s_worker_core = -1;

static void trino_worker_main(void* unused)
{
    (void)unused;
    for (;;) {
        LightSemaphore_Acquire(&s_job_ready, 1);
        if (s_worker_quit) {
            return;
        }
        LightLock_Lock(&s_job_lock);
        trino_job_fn fn = s_jobs[s_job_head % TRINO_JOBS].fn;
        uint32_t arg = s_jobs[s_job_head % TRINO_JOBS].arg;
        s_job_head++;
        LightLock_Unlock(&s_job_lock);
        fn(arg);
        if (__atomic_sub_fetch(&s_job_pending, 1, __ATOMIC_ACQ_REL) == 0) {
            LightEvent_Signal(&s_job_idle);
        }
    }
}

// Start the worker (once; later calls return the same core): core 2 on a
// New 3DS, else core 1 with 30% of its time. Returns the core, or -1 when
// none was granted (jobs then run inline). Call after trino_init.
int32_t trino_worker_start(uint32_t new3ds)
{
    if (s_worker) {
        return s_worker_core;
    }
    LightLock_Init(&s_job_lock);
    LightSemaphore_Init(&s_job_ready, 0, TRINO_JOBS + 1);
    LightEvent_Init(&s_job_idle, RESET_ONESHOT);
    s32 prio = 0x30;
    svcGetThreadPriority(&prio, CUR_THREAD_HANDLE);
    // Application threads take 0x18..0x3F; on its own core the worker runs
    // at the main thread's priority.
    if (prio < 0x18) prio = 0x18;
    if (prio > 0x3F) prio = 0x3F;
    s_worker = trino_second_core_thread(trino_worker_main, 32 * 1024, prio, new3ds != 0,
                                        &s_worker_core);
    return s_worker ? s_worker_core : -1;
}

// Queue job(arg) for the worker. Returns 0 (nothing queued: the caller
// runs it) without a worker or with the ring full.
uint32_t trino_worker_submit(trino_job_fn job, uint32_t arg)
{
    if (!s_worker) {
        return 0;
    }
    LightLock_Lock(&s_job_lock);
    if (s_job_tail - s_job_head >= TRINO_JOBS) {
        LightLock_Unlock(&s_job_lock);
        return 0;
    }
    s_jobs[s_job_tail % TRINO_JOBS].fn = job;
    s_jobs[s_job_tail % TRINO_JOBS].arg = arg;
    s_job_tail++;
    LightLock_Unlock(&s_job_lock);
    // Counted before the worker can see it, so the count never underflows.
    __atomic_add_fetch(&s_job_pending, 1, __ATOMIC_ACQ_REL);
    LightSemaphore_Release(&s_job_ready, 1);
    return 1;
}

// Block until every queued job has run (a stale signal from an earlier
// batch only loops once more).
void trino_worker_wait(void)
{
    if (!s_worker) {
        return;
    }
    while (__atomic_load_n(&s_job_pending, __ATOMIC_ACQUIRE) != 0) {
        LightEvent_Wait(&s_job_idle);
    }
}

// Stop and join the worker (teardown).
static void trino_worker_stop(void)
{
    if (!s_worker) {
        return;
    }
    trino_worker_wait();
    s_worker_quit = true;
    LightSemaphore_Release(&s_job_ready, 1);
    threadJoin(s_worker, U64_MAX);
    threadFree(s_worker);
    s_worker = NULL;
}

// svcOutputDebugString: shows up in Azahar/Citra logs and GDB; also on the
// 3dslink host when the app was launched by it.
void trino_log(const char* msg)
{
    size_t len = strlen(msg);
    svcOutputDebugString(msg, len);
    trino_link_write(msg, len);
}

// Rust panic lands here: report and halt (responding to HOME so the
// emulator/console can still close the app).
void trino_panic(const char* msg)
{
    svcOutputDebugString("TRINO_PANIC: ", 13);
    svcOutputDebugString(msg, strlen(msg));
    trino_link_write("TRINO_PANIC: ", 13);
    trino_link_write(msg, strlen(msg));
    trino_link_write("\n", 1);
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
    trino_tnl_leave();
    C2D_SceneBegin(bottom ? s_bottom : s_top);
    s_proj = bottom ? &s_proj_bottom : &s_proj_top;
}

// ---------------------------------------------------------------------------
// Stereoscopic 3D (issue #63, ADR 0032). The Rust side records the top
// screen's draws and replays them once per eye: the left eye into s_top,
// then trino_stereo_eye(1, clear) moves the draws to the right eye's
// target. trino_stereo_mode follows whether the frame drew two eyes, so a
// mono frame never shows a stale right eye.

// The 3D slider, 16.16 fixed point 0..1 (0 on a 2DS, whose slider state
// stays 0).
uint32_t trino_stereo_slider(void)
{
    float s = osGet3DSliderState();
    if (!(s > 0.0f)) return 0;
    if (s > 1.0f) s = 1.0f;
    return (uint32_t)(s * 65536.0f);
}

// 1 = the frame being drawn shows two eyes, 0 = one. Call before
// trino_frame_end; only a change touches the display (gfxSet3D).
void trino_stereo_mode(uint32_t on)
{
    on = on ? 1 : 0;
    if (on != (uint32_t)s_stereo_on) {
        s_stereo_on = (int)on;
        gfxSet3D(on != 0);
    }
}

// Later top-screen draws go to the right eye (eye = 1, cleared to `rgba`
// first) or the left one (0). Returns 0 when the right eye's target can't
// be made (no VRAM): the caller then keeps the frame mono.
uint32_t trino_stereo_eye(uint32_t eye, uint32_t rgba8888)
{
    trino_tnl_leave();
    if (!eye) {
        C2D_SceneBegin(s_top);
        s_proj = &s_proj_top;
        return 1;
    }
    if (!s_top_right) {
        s_top_right = C3D_RenderTargetCreate(GSP_SCREEN_WIDTH, GSP_SCREEN_HEIGHT_TOP,
                                             GPU_RB_RGBA8, GPU_RB_DEPTH24_STENCIL8);
        if (!s_top_right) return 0;
        C3D_RenderTargetSetOutput(
            s_top_right, GFX_TOP, GFX_RIGHT,
            GX_TRANSFER_FLIP_VERT(0) | GX_TRANSFER_OUT_TILED(0) | GX_TRANSFER_RAW_COPY(0) |
                GX_TRANSFER_IN_FORMAT(GX_TRANSFER_FMT_RGBA8) |
                GX_TRANSFER_OUT_FORMAT(GX_TRANSFER_FMT_RGB8) |
                GX_TRANSFER_SCALING(GX_TRANSFER_SCALE_NO));
    }
    C2D_TargetClear(s_top_right, C2D_Color32((rgba8888 >> 24) & 0xFF, (rgba8888 >> 16) & 0xFF,
                                             (rgba8888 >> 8) & 0xFF, 0xFF));
    C2D_SceneBegin(s_top_right);
    s_proj = &s_proj_top;
    return 1;
}

// A frame ended since the debug readback last waited for the display
// (trino_debug_landed).
static int s_frame_unseen = 1;

void trino_frame_end(void)
{
    trino_tnl_leave();
    C3D_FrameEnd(0);
    s_frame_unseen = 1;
}

// ---------------------------------------------------------------------------
// Sprites — one .t3x per asset, image 0 of its sheet.

// Texture formats (issue #52): tex3ds writes any PICA200 format and citro2d /
// citro3d sample it as is: ETC1 opaque, ETC1A4 with its 4-bit alpha, L8 gray
// opaque, LA8 gray with alpha. A8 (and A4) is the exception: the PICA200
// samples it as (0, 0, 0, a), while the engine's A8 is a white mask (the PC
// preview, `trino_asset_pipeline::texture`). Mask textures therefore enter a
// TexEnv through the "1 - color" operand (black -> white), and a sprite blit
// of one uses citro2d's solid tint mode (the tint's color, the texel's alpha).
static int tex_is_mask(const C3D_Tex* t)
{
    // `fmt` is a 4-bit enum bitfield that gcc makes signed: GPU_A8 (8)
    // reads back as -8, so compare its low 4 bits.
    unsigned fmt = (unsigned)t->fmt & 15u;
    return fmt == GPU_A8 || fmt == GPU_A4;
}

static GPU_TEVOP_RGB tex_rgb_op(const C3D_Tex* t)
{
    return tex_is_mask(t) ? GPU_TEVOP_RGB_ONE_MINUS_SRC_COLOR : GPU_TEVOP_RGB_SRC_COLOR;
}

void* trino_sprite_load(const char* romfs_path)
{
    return C2D_SpriteSheetLoad(romfs_path);
}

// A .t3x already in memory (an asset group's bundle entry): citro2d copies
// the texels to linear memory, the caller frees `data`.
void* trino_sprite_load_mem(const void* data, uint32_t size)
{
    return C2D_SpriteSheetLoadFromMem(data, size);
}

// Free a sheet (an unloaded group's) once the GPU is past the frames that
// drew it (the renderer's grave). Unbinds it everywhere a later draw could
// find it: the detail slot, citro3d's texture units, citro2d's current
// texture (C2D_Prepare forgets it).
void trino_sprite_free(void* sheet)
{
    if (!sheet) return;
    if (s_detail_sheet == (C2D_SpriteSheet)sheet) s_detail_sheet = NULL;
    C2D_SpriteSheetFree((C2D_SpriteSheet)sheet);
    for (int i = 0; i < 3; i++) C3D_TexBind(i, NULL);
    C2D_Prepare();
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
    trino_tnl_leave();
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

    if (tex_is_mask(img.tex)) {
        // A white mask times the tint: the solid tint mode (blend 1) takes
        // the tint's color and the texel's alpha, then back to multiply.
        C2D_ImageTint tint;
        C2D_PlainImageTint(&tint,
                           C2D_Color32((p->tint >> 24) & 0xFF, (p->tint >> 16) & 0xFF,
                                       (p->tint >> 8) & 0xFF, p->tint & 0xFF),
                           1.0f);
        C2D_SetTintMode(C2D_TintSolid);
        C2D_DrawImage(img, &params, &tint);
        C2D_SetTintMode(C2D_TintMult);
    } else if (p->tint != 0xFFFFFFFFu) {
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
    trino_tnl_leave();
    uint32_t c = f->color;
    C2D_DrawRectSolid(f->x, f->y, f->depth, f->w, f->h,
                      C2D_Color32((c >> 24) & 0xFF, (c >> 16) & 0xFF, (c >> 8) & 0xFF, c & 0xFF));
}

// ---------------------------------------------------------------------------
// 3D triangles: the engine transforms and lights on the CPU
// (trino_core::render3d); citro2d only rasterizes gouraud-shaded tris.

// The stencil's part in a shadow draw. Values: 0 free, 1 a receiver
// marked for the GPU T&L shadow on it, 2 shadowed (no more shadow there).
//   1 once:    CPU shadows (already cut to their receiver): where not yet
//              shadowed, and mark it so;
//   2 mark:    a receiver's quad, no color: free -> 1 where it shows;
//   3 on mark: a GPU T&L shadow: 1 -> 2, nowhere else (cut to the mark);
//   4 unmark:  the receiver's quad again: 1 -> 0 (what no shadow covered).
// 0: no stencil (blobs, which blend over each other like the N64's).
static void trino_shadow_stencil(int mode)
{
    switch (mode) {
    case 1:
        C3D_StencilTest(true, GPU_NOTEQUAL, 2, 0xFF, 0xFF);
        C3D_StencilOp(GPU_STENCIL_KEEP, GPU_STENCIL_KEEP, GPU_STENCIL_REPLACE);
        break;
    case 2:
        C3D_StencilTest(true, GPU_EQUAL, 0, 0xFF, 0xFF);
        C3D_StencilOp(GPU_STENCIL_KEEP, GPU_STENCIL_KEEP, GPU_STENCIL_INCR);
        break;
    case 3:
        C3D_StencilTest(true, GPU_EQUAL, 1, 0xFF, 0xFF);
        C3D_StencilOp(GPU_STENCIL_KEEP, GPU_STENCIL_KEEP, GPU_STENCIL_INCR);
        break;
    case 4:
        C3D_StencilTest(true, GPU_EQUAL, 1, 0xFF, 0xFF);
        C3D_StencilOp(GPU_STENCIL_KEEP, GPU_STENCIL_KEEP, GPU_STENCIL_DECR);
        break;
    default:
        C3D_StencilTest(false, GPU_ALWAYS, 0, 0xFF, 0x00);
        C3D_StencilOp(GPU_STENCIL_KEEP, GPU_STENCIL_KEEP, GPU_STENCIL_KEEP);
        break;
    }
    // The marks write no color; the rest blend the shadow color. Depth is
    // tested (GEQUAL: citro2d's nearer-is-larger) and never written.
    C3D_DepthTest(true, GPU_GEQUAL, (mode == 2 || mode == 4) ? 0 : GPU_WRITE_COLOR);
}

// Back to citro2d's state after a shadow draw (C2D_Prepare restores the
// depth test; the stencil it never touches).
static void trino_shadow_leave(void)
{
    C3D_StencilTest(false, GPU_ALWAYS, 0, 0xFF, 0x00);
    C3D_StencilOp(GPU_STENCIL_KEEP, GPU_STENCIL_KEEP, GPU_STENCIL_KEEP);
    C3D_DepthTest(true, GPU_GEQUAL, GPU_WRITE_ALL);
}

// The blob texture (trino_core::shadow::blob_texels): `size`² 8-bit alpha
// values, row-major, made an A8 texture (8x8 tiles, Morton order within;
// the falloff is round, so which way up the rows go doesn't matter). Made
// again when the softness changes (`begin_frame` waited for the GPU).
void trino_shadow_blob(const uint8_t* texels, uint32_t size)
{
    if (s_blob_ready && (s_blob.width != size || s_blob.height != size)) {
        C3D_TexDelete(&s_blob);
        s_blob_ready = 0;
    }
    if (!s_blob_ready) {
        if (!C3D_TexInit(&s_blob, (u16)size, (u16)size, GPU_A8)) return;
        C3D_TexSetFilter(&s_blob, GPU_LINEAR, GPU_LINEAR);
        C3D_TexSetWrap(&s_blob, GPU_CLAMP_TO_EDGE, GPU_CLAMP_TO_EDGE);
        s_blob_ready = 1;
    }
    uint8_t* dst = (uint8_t*)s_blob.data;
    for (uint32_t y = 0; y < size; y++) {
        for (uint32_t x = 0; x < size; x++) {
            uint32_t tile = ((y >> 3) * (size >> 3) + (x >> 3)) * 64;
            uint32_t m = (x & 1) | ((y & 1) << 1) | ((x & 2) << 1) | ((y & 2) << 2) |
                         ((x & 4) << 2) | ((y & 4) << 3);
            dst[tile + m] = texels[y * size + x];
        }
    }
    C3D_TexFlush(&s_blob);
}

// Sampling of the next trino_3d_draw calls: `flags` bit 1 point filter,
// bits 4..7 the detail layer's repeats per base repeat (log2); `detail`
// (NULL = none) is a sprite sheet multiplied over the texture.
void trino_3d_material(void* detail, uint32_t flags, uint32_t scroll)
{
    s_point = (flags >> 1) & 1;
    s_detail_sheet = (C2D_SpriteSheet)detail;
    s_detail_scale = (float)(1u << ((flags >> 4) & 15));
    s_water = (flags >> 8) & 1;
    s_shadow = (flags >> 9) & 1;
    s_stencil = s_shadow ? (int)((flags >> 10) & 7) : 0;
    s_blob_on = s_shadow && ((flags >> 13) & 1) && s_blob_ready;
    s_detail_off[0] = s_water ? (float)(scroll >> 16) / 65536.0f : 0.0f;
    s_detail_off[1] = s_water ? (float)(scroll & 0xFFFF) / 65536.0f : 0.0f;
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
    trino_tnl_leave();

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
    C3D_FVUnifSet(GPU_VERTEX_SHADER, s_tri3d_detailoff, s_detail_off[0], s_detail_off[1], 0.0f,
                  0.0f);
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
        C3D_TexEnvOpRgb(env, tex_rgb_op(img.tex), GPU_TEVOP_RGB_SRC_COLOR, GPU_TEVOP_RGB_SRC_COLOR);
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
        C2D_Image dimg = C2D_SpriteSheetGetImage(s_detail_sheet, 0);
        GPU_TEXTURE_FILTER_PARAM filter = s_point ? GPU_NEAREST : GPU_LINEAR;
        C3D_TexSetFilter(dimg.tex, filter, filter);
        C3D_TexSetWrap(dimg.tex, GPU_REPEAT, GPU_REPEAT);
        C3D_TexBind(1, dimg.tex);
        C3D_TexEnv* env1 = C3D_GetTexEnv(1);
        if (s_water) {
            // Water: stage 0 blends the layers half and half (the N64's
            // ENV_ALPHA lerp), stage 1 multiplies by the vertex color; the
            // alpha is the vertex's.
            C3D_TexEnvInit(env);
            C3D_TexEnvSrc(env, C3D_RGB, GPU_TEXTURE1, GPU_TEXTURE0, GPU_CONSTANT);
            C3D_TexEnvOpRgb(env, tex_rgb_op(dimg.tex), tex_rgb_op(img.tex), GPU_TEVOP_RGB_SRC_COLOR);
            C3D_TexEnvFunc(env, C3D_RGB, GPU_INTERPOLATE);
            C3D_TexEnvColor(env, 0x80808080);
            C3D_TexEnvSrc(env, C3D_Alpha, GPU_PRIMARY_COLOR, 0, 0);
            C3D_TexEnvFunc(env, C3D_Alpha, GPU_REPLACE);
            C3D_TexEnvSrc(env1, C3D_RGB, GPU_PREVIOUS, GPU_PRIMARY_COLOR, 0);
            C3D_TexEnvFunc(env1, C3D_RGB, GPU_MODULATE);
        } else {
            // Stage 1: the lit texel times the detail texel (alpha stays).
            C3D_TexEnvSrc(env1, C3D_RGB, GPU_PREVIOUS, GPU_TEXTURE1, 0);
            C3D_TexEnvOpRgb(env1, GPU_TEVOP_RGB_SRC_COLOR, tex_rgb_op(dimg.tex), GPU_TEVOP_RGB_SRC_COLOR);
            C3D_TexEnvFunc(env1, C3D_RGB, GPU_MODULATE);
        }
        C3D_TexEnvSrc(env1, C3D_Alpha, GPU_PREVIOUS, 0, 0);
        C3D_TexEnvFunc(env1, C3D_Alpha, GPU_REPLACE);
    } else if (s_water && sheet) {
        // Water with one layer: the texel's color, the vertex's alpha.
        C3D_TexEnvSrc(env, C3D_Alpha, GPU_PRIMARY_COLOR, 0, 0);
        C3D_TexEnvFunc(env, C3D_Alpha, GPU_REPLACE);
    }
    if (s_water) {
        // Translucent: blended over what the batch drew, depth tested but
        // not written (C2D_Prepare below restores citro2d's depth state).
        C3D_AlphaTest(false, GPU_ALWAYS, 0);
        C3D_DepthTest(true, GPU_GEQUAL, GPU_WRITE_COLOR);
    }
    if (s_shadow) {
        // The shadow color by the vertex alpha (times the blob's).
        C3D_AlphaTest(false, GPU_ALWAYS, 0);
        if (s_blob_on) {
            C3D_TexBind(0, &s_blob);
            C3D_TexEnvInit(env);
            C3D_TexEnvSrc(env, C3D_RGB, GPU_PRIMARY_COLOR, 0, 0);
            C3D_TexEnvFunc(env, C3D_RGB, GPU_REPLACE);
            C3D_TexEnvSrc(env, C3D_Alpha, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
            C3D_TexEnvFunc(env, C3D_Alpha, GPU_MODULATE);
        }
        trino_shadow_stencil(s_stencil);
    }
    C3D_DrawArrays(GPU_TRIANGLES, s_vused, count);
    s_vused += count;
    if (s_shadow) trino_shadow_leave();
    // Hand the GPU back to citro2d (program, attributes, buffers, texenv).
    C2D_Prepare();
}

// ---------------------------------------------------------------------------
// GPU T&L for static meshes (the draws trino_core::render3d::gpu_check
// accepts): the PICA200's vertex units transform, light and project; the
// CPU only writes 44 floats of uniforms per draw.

#define TNL_VERTEX_BYTES 28 // x, y, z, nx, ny, nz (floats), r, g, b, a (bytes)

typedef struct {
    void* verts;     // linear memory, TNL_VERTEX_BYTES each
    u16* indices;    // linear memory
    uint32_t count;  // indices
} trino_gpu_mesh_t;

// Copy a mesh to linear memory for the GPU T&L path: `nverts` vertices of
// TNL_VERTEX_BYTES and `nidx` u16 indices (triangles). NULL when linear
// memory is short (the renderer keeps the CPU path for that mesh).
void* trino_gpu_mesh(const void* verts, uint32_t nverts, const uint16_t* idx, uint32_t nidx)
{
    trino_gpu_mesh_t* m = (trino_gpu_mesh_t*)malloc(sizeof(trino_gpu_mesh_t));
    if (!m) return NULL;
    m->verts = linearAlloc(nverts * TNL_VERTEX_BYTES);
    m->indices = (u16*)linearAlloc(nidx * sizeof(u16));
    if (!m->verts || !m->indices) {
        if (m->verts) linearFree(m->verts);
        if (m->indices) linearFree(m->indices);
        free(m);
        return NULL;
    }
    memcpy(m->verts, verts, nverts * TNL_VERTEX_BYTES);
    memcpy(m->indices, idx, nidx * sizeof(u16));
    GSPGPU_FlushDataCache(m->verts, nverts * TNL_VERTEX_BYTES);
    GSPGPU_FlushDataCache(m->indices, nidx * sizeof(u16));
    m->count = nidx;
    return m;
}

// Free a trino_gpu_mesh or trino_water_mesh (same layout) once the GPU is
// past the frames that drew it.
void trino_gpu_mesh_free(void* mesh)
{
    trino_gpu_mesh_t* m = (trino_gpu_mesh_t*)mesh;
    if (!m) return;
    linearFree(m->verts);
    linearFree(m->indices);
    free(m);
}

// Draw a trino_gpu_mesh. `u`: the 4x4 matrix to the screen's pixels and
// per-vertex depth band, times the view depth (16 floats, rows), then
// light direction, ambient, diffuse, tint, emissive, the fog row and the
// fog color (4 floats each).
// The shim puts the current screen's tilt projection in front of the
// matrix. flags bit 0: cull back faces.
void trino_gpu_draw(void* mesh, const float* u, uint32_t flags)
{
    trino_gpu_mesh_t* m = (trino_gpu_mesh_t*)mesh;
    if (!m) return;
    C3D_Mtx screen, mvp;
    for (int r = 0; r < 4; r++) {
        screen.r[r].x = u[r * 4 + 0];
        screen.r[r].y = u[r * 4 + 1];
        screen.r[r].z = u[r * 4 + 2];
        screen.r[r].w = u[r * 4 + 3];
    }
    Mtx_Multiply(&mvp, s_proj, &screen);

    if (!s_tnl_on) {
        // Enter the T&L state once per run of GPU draws.
        C2D_Flush();
        C3D_BindProgram(&s_tnl3d);
        C3D_AttrInfo* attr = C3D_GetAttrInfo();
        AttrInfo_Init(attr);
        AttrInfo_AddLoader(attr, 0, GPU_FLOAT, 3);         // position
        AttrInfo_AddLoader(attr, 1, GPU_FLOAT, 3);         // normal
        AttrInfo_AddLoader(attr, 2, GPU_UNSIGNED_BYTE, 4); // color
        C3D_AlphaTest(false, GPU_ALWAYS, 0);
        C3D_TexEnv* env = C3D_GetTexEnv(0);
        C3D_TexEnvInit(env);
        C3D_TexEnvSrc(env, C3D_Both, GPU_PRIMARY_COLOR, 0, 0);
        C3D_TexEnvFunc(env, C3D_Both, GPU_REPLACE);
        for (int i = 1; i < 6; i++) {
            C3D_TexEnvInit(C3D_GetTexEnv(i));
        }
        s_tnl_on = 1;
    }
    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, s_tnl3d_unif[0], &mvp);
    for (int i = 1; i < TNL_UNIFORMS; i++) {
        const float* v = u + 16 + (i - 1) * 4;
        C3D_FVUnifSet(GPU_VERTEX_SHADER, s_tnl3d_unif[i], v[0], v[1], v[2], v[3]);
    }
    C3D_BufInfo* buf = C3D_GetBufInfo();
    BufInfo_Init(buf);
    BufInfo_Add(buf, m->verts, TNL_VERTEX_BYTES, 3, 0x210);
    // The tilt projection mirrors the screen: the engine's front faces
    // arrive clockwise.
    C3D_CullFace((flags & 1) ? TRINO_TNL_CULL : GPU_CULL_NONE);
    // flags bit 1: a planar shadow (ADR 0025), bits 4..6 its stencil part
    // (trino_shadow_stencil): blended, depth tested, not written.
    if (flags & 2) trino_shadow_stencil((int)((flags >> 4) & 7));
    C3D_DrawElements(GPU_TRIANGLES, m->count, C3D_UNSIGNED_SHORT, m->indices);
    if (flags & 2) trino_shadow_leave();
}

// ---------------------------------------------------------------------------
// GPU water (ADR 0015): a water mesh in linear memory (x, y, z, u, v floats,
// r, g, b, a bytes), drawn by water.v.pica with the material set by
// trino_3d_material (the detail layer, the point filter, water on): the
// layers blended half and half times the vertex color, blended over the
// frame, depth tested but not written.

#define WATER_VERTEX_BYTES 24

void* trino_water_mesh(const void* verts, uint32_t nverts, const uint16_t* idx, uint32_t nidx)
{
    trino_gpu_mesh_t* m = (trino_gpu_mesh_t*)malloc(sizeof(trino_gpu_mesh_t));
    if (!m) return NULL;
    m->verts = linearAlloc(nverts * WATER_VERTEX_BYTES);
    m->indices = (u16*)linearAlloc(nidx * sizeof(u16));
    if (!m->verts || !m->indices) {
        if (m->verts) linearFree(m->verts);
        if (m->indices) linearFree(m->indices);
        free(m);
        return NULL;
    }
    memcpy(m->verts, verts, nverts * WATER_VERTEX_BYTES);
    memcpy(m->indices, idx, nidx * sizeof(u16));
    GSPGPU_FlushDataCache(m->verts, nverts * WATER_VERTEX_BYTES);
    GSPGPU_FlushDataCache(m->indices, nidx * sizeof(u16));
    m->count = nidx;
    return m;
}

// Draw a trino_water_mesh. `u`: the 4x4 matrix from the world to the
// screen's pixels and depth band times the view depth (16 floats, rows),
// then the model matrix's 3 rows and 19 vectors in water.v.pica's order
// (wdir .. detailoff): 104 floats. `sheet` = the base texture (NULL =
// vertex colors). flags bit 0: cull back faces.
void trino_water_draw(void* mesh, const float* u, void* sheet, uint32_t flags)
{
    trino_gpu_mesh_t* m = (trino_gpu_mesh_t*)mesh;
    if (!m) return;
    trino_tnl_leave();
    C3D_Mtx screen, mvp;
    for (int r = 0; r < 4; r++) {
        screen.r[r].x = u[r * 4 + 0];
        screen.r[r].y = u[r * 4 + 1];
        screen.r[r].z = u[r * 4 + 2];
        screen.r[r].w = u[r * 4 + 3];
    }
    Mtx_Multiply(&mvp, s_proj, &screen);

    C2D_Flush();
    C3D_BindProgram(&s_water_prog);
    C3D_FVUnifMtx4x4(GPU_VERTEX_SHADER, s_water_unif[0], &mvp);
    for (int r = 0; r < 3; r++) {
        const float* v = u + 16 + r * 4;
        C3D_FVUnifSet(GPU_VERTEX_SHADER, s_water_unif[1] + r, v[0], v[1], v[2], v[3]);
    }
    for (int i = 2; i < 21; i++) {
        const float* v = u + 28 + (i - 2) * 4;
        C3D_FVUnifSet(GPU_VERTEX_SHADER, s_water_unif[i], v[0], v[1], v[2], v[3]);
    }
    float l = 0.0f, t = 0.0f, w = 1.0f, h = 1.0f;
    C2D_Image img = {0};
    if (sheet) {
        img = C2D_SpriteSheetGetImage((C2D_SpriteSheet)sheet, 0);
        l = img.subtex->left;
        t = img.subtex->top;
        w = img.subtex->right - l;
        h = img.subtex->bottom - t;
    }
    C3D_FVUnifSet(GPU_VERTEX_SHADER, s_water_unif[21], l, t, w, h);
    C3D_FVUnifSet(GPU_VERTEX_SHADER, s_water_unif[22], s_detail_scale, s_detail_scale, 0.0f,
                  0.0f);

    C3D_AttrInfo* attr = C3D_GetAttrInfo();
    AttrInfo_Init(attr);
    AttrInfo_AddLoader(attr, 0, GPU_FLOAT, 3);         // position
    AttrInfo_AddLoader(attr, 1, GPU_FLOAT, 2);         // uv
    AttrInfo_AddLoader(attr, 2, GPU_UNSIGNED_BYTE, 4); // color
    C3D_BufInfo* buf = C3D_GetBufInfo();
    BufInfo_Init(buf);
    BufInfo_Add(buf, m->verts, WATER_VERTEX_BYTES, 3, 0x210);

    C3D_TexEnv* env = C3D_GetTexEnv(0);
    C3D_TexEnvInit(env);
    for (int i = 1; i < 6; i++) {
        C3D_TexEnvInit(C3D_GetTexEnv(i));
    }
    GPU_TEXTURE_FILTER_PARAM filter = s_point ? GPU_NEAREST : GPU_LINEAR;
    int detail = sheet && s_detail_sheet;
    if (sheet) {
        C3D_TexSetFilter(img.tex, filter, filter);
        C3D_TexSetWrap(img.tex, GPU_REPEAT, GPU_REPEAT);
        C3D_TexBind(0, img.tex);
    }
    if (detail) {
        C2D_Image dimg = C2D_SpriteSheetGetImage(s_detail_sheet, 0);
        C3D_TexSetFilter(dimg.tex, filter, filter);
        C3D_TexSetWrap(dimg.tex, GPU_REPEAT, GPU_REPEAT);
        C3D_TexBind(1, dimg.tex);
        // Stage 0: the layers half and half; stage 1: times the vertex
        // color. The alpha is the vertex's.
        C3D_TexEnvSrc(env, C3D_RGB, GPU_TEXTURE1, GPU_TEXTURE0, GPU_CONSTANT);
        C3D_TexEnvOpRgb(env, tex_rgb_op(dimg.tex), tex_rgb_op(img.tex), GPU_TEVOP_RGB_SRC_COLOR);
        C3D_TexEnvFunc(env, C3D_RGB, GPU_INTERPOLATE);
        C3D_TexEnvColor(env, 0x80808080);
        C3D_TexEnvSrc(env, C3D_Alpha, GPU_PRIMARY_COLOR, 0, 0);
        C3D_TexEnvFunc(env, C3D_Alpha, GPU_REPLACE);
        C3D_TexEnv* env1 = C3D_GetTexEnv(1);
        C3D_TexEnvSrc(env1, C3D_RGB, GPU_PREVIOUS, GPU_PRIMARY_COLOR, 0);
        C3D_TexEnvFunc(env1, C3D_RGB, GPU_MODULATE);
        C3D_TexEnvSrc(env1, C3D_Alpha, GPU_PREVIOUS, 0, 0);
        C3D_TexEnvFunc(env1, C3D_Alpha, GPU_REPLACE);
    } else if (sheet) {
        C3D_TexEnvSrc(env, C3D_RGB, GPU_TEXTURE0, GPU_PRIMARY_COLOR, 0);
        C3D_TexEnvOpRgb(env, tex_rgb_op(img.tex), GPU_TEVOP_RGB_SRC_COLOR, GPU_TEVOP_RGB_SRC_COLOR);
        C3D_TexEnvFunc(env, C3D_RGB, GPU_MODULATE);
        C3D_TexEnvSrc(env, C3D_Alpha, GPU_PRIMARY_COLOR, 0, 0);
        C3D_TexEnvFunc(env, C3D_Alpha, GPU_REPLACE);
    } else {
        C3D_TexEnvSrc(env, C3D_Both, GPU_PRIMARY_COLOR, 0, 0);
        C3D_TexEnvFunc(env, C3D_Both, GPU_REPLACE);
    }
    C3D_AlphaTest(false, GPU_ALWAYS, 0);
    C3D_DepthTest(true, GPU_GEQUAL, GPU_WRITE_COLOR);
    // The tilt projection mirrors the screen (see trino_gpu_draw).
    C3D_CullFace((flags & 1) ? TRINO_TNL_CULL : GPU_CULL_NONE);
    C3D_DrawElements(GPU_TRIANGLES, m->count, C3D_UNSIGNED_SHORT, m->indices);
    C3D_CullFace(GPU_CULL_NONE);
    // Hand the GPU back to citro2d (program, attributes, texenv, depth).
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

// Wait two vblanks for the last presented frame's display transfer to land,
// once per frame: a probe reading 960 pixels used to wait 33 ms for each
// one (64 s with the screen frozen, measured in Azahar).
static void trino_debug_landed(void)
{
    if (s_frame_unseen) {
        gspWaitForVBlank();
        gspWaitForVBlank();
        s_frame_unseen = 0;
    }
}

// Test hook: RGBA8888 of the top screen (bottom = 0) or the touch screen
// (bottom = 1) at (x, y) (landscape, y down) from the LCD back buffer; 0 if
// unavailable. Waits two vblanks so the frame's display transfer has
// landed (once per presented frame: further reads of the same frame skip
// the wait, see trino_debug_landed); callers draw the probed frame several
// times so both LCD buffers hold it. The LCD buffer is column-major,
// bottom-up.
uint32_t trino_debug_pixel(uint32_t x, uint32_t y, uint32_t screen)
{
    trino_tnl_leave();
    // 0 the top screen (its left eye), 1 the touch screen, 2 the top
    // screen's right eye (issue #63).
    uint32_t bottom = screen == 1;
    if (x >= (bottom ? 320u : 400u) || y >= 240) return 0;
    trino_debug_landed();
    u16 w = 0, h = 0;
    gfxScreen_t scr = bottom ? GFX_BOTTOM : GFX_TOP;
    const u8* fb = gfxGetFramebuffer(scr, screen == 2 ? GFX_RIGHT : GFX_LEFT, &w, &h);
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
void trino_debug_fb_dump(uint32_t screen)
{
    trino_tnl_leave();
    trino_debug_landed();
    // The screens of trino_debug_pixel (2 = the top screen's right eye).
    uint32_t bottom = screen == 1;
    u16 w = 0, h = 0;
    gfxScreen_t scr = bottom ? GFX_BOTTOM : GFX_TOP;
    const uint32_t width = bottom ? 320 : 400;
    const u8* fb = gfxGetFramebuffer(scr, screen == 2 ? GFX_RIGHT : GFX_LEFT, &w, &h);
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
    // New 3DS only (hidInit starts ir:rst there and hidScanInput merges
    // its keys); always 0 on an Old 3DS.
    if (k & KEY_ZL) out |= 1u << 12;   // L2
    if (k & KEY_ZR) out |= 1u << 13;   // R2
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

// Packed C-stick (New 3DS), like trino_joypad_stick. irrstCstickRead copies
// what hidScanInput read, so it is 0 on an Old 3DS (no ir:rst).
uint32_t trino_cstick(void)
{
    circlePosition pos;
    hidCstickRead(&pos);
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
// Audio (issue #53) — `T3AU` sounds through ndsp: DSP-ADPCM (decoded by the
// DSP itself, ~3.5x smaller than PCM16) or PCM16, mono. Layout, written by
// `cargo xtask assets 3ds` (trino_asset_pipeline::audio::n3ds_container),
// little-endian:
//    0 "T3AU"   4 u8 codec (0 PCM16, 1 DSP-ADPCM)   5 u8 bus
//    6 u8 flags (bit 0 loop, bit 1 stream)   7 u8 channels (1)
//    8 u32 rate   12 u32 samples   16 u32 data bytes
//   20 u32 chunk samples (streamed: samples per wave buffer, 0 = whole)
//   24 u32 context count   28 16 x u16 DSP-ADPCM coefficients
//   60 a context per chunk (u16 predictor/scale, s16 last sample, s16 the
//      one before, u16 0: what ndspAdpcmData holds), then the data.
// Whole sounds load into linear memory at boot. A streamed one keeps only
// its header and contexts: each playback reads its data from the RomFS in
// chunks into three rotating linear buffers, refilled by the audio thread
// (woken by ndsp every audio frame). Every wave buffer carries its ADPCM
// context, so loops and chunk seams decode exactly like the bake's preview.

#define T3AU_HEADER 60
#define TRINO_NDSP_CHANNELS 16
#define TRINO_STREAM_BUFS 3
#define TRINO_MAX_STREAMS 4

typedef struct {
    uint32_t sample_rate;
    uint32_t nsamples;
    uint32_t data_bytes;
    uint32_t chunk_samples;   // streamed: samples per wave buffer
    uint32_t ncontexts;
    uint8_t adpcm, bus, looped, stream;
    u16 coefs[16];
    ndspAdpcmData* contexts;  // one per chunk (malloc'd), NULL when none
    ndspAdpcmData start;      // the decoder state at sample 0
    void* data;               // linearAlloc'd whole sound; NULL when streamed
    uint32_t data_offset;     // streamed: where the data starts in the file
    char path[64];            // streamed: the RomFS file
} trino_wav_t;

typedef struct {
    int active;               // a playback owns the slot
    int ended;                // non-looping: the last chunk is queued
    uint32_t channel;
    trino_wav_t* wav;
    FILE* f;
    uint32_t next_chunk, nchunks;
    int looped;
    uint32_t buf_bytes;
    void* buf[TRINO_STREAM_BUFS];
    ndspWaveBuf wb[TRINO_STREAM_BUFS];
    ndspAdpcmData ctx[TRINO_STREAM_BUFS];
} trino_stream_t;

static ndspWaveBuf s_wavebufs[TRINO_NDSP_CHANNELS];
static ndspAdpcmData s_wavectx[TRINO_NDSP_CHANNELS];
static trino_stream_t s_streams[TRINO_MAX_STREAMS];
static int s_stream_of[TRINO_NDSP_CHANNELS];
// Held by the audio thread while it refills, and by the main thread while
// it starts, stops or releases a stream (the only thread that does).
static LightLock s_audio_lock;
static LightEvent s_audio_wake;
static Thread s_audio_thread = NULL;
static volatile int s_audio_quit = 0;
static int s_audio_core = -2;
// Counters behind trino_audio_stat.
static uint32_t s_resident = 0;
static volatile uint32_t s_underruns = 0;
static volatile uint32_t s_streamed = 0;

static uint32_t rd32(const uint8_t* p)
{
    return (uint32_t)p[0] | (uint32_t)p[1] << 8 | (uint32_t)p[2] << 16 | (uint32_t)p[3] << 24;
}

static int16_t rd16(const uint8_t* p)
{
    return (int16_t)((uint16_t)p[0] | (uint16_t)p[1] << 8);
}

void* trino_wav_load(const char* romfs_path)
{
    FILE* f = fopen(romfs_path, "rb");
    if (!f) return NULL;
    uint8_t h[T3AU_HEADER];
    if (fread(h, 1, sizeof(h), f) != sizeof(h) || memcmp(h, "T3AU", 4) != 0 || h[4] > 1 ||
        h[7] != 1 || strlen(romfs_path) >= sizeof(((trino_wav_t*)0)->path)) {
        fclose(f);
        return NULL;
    }
    trino_wav_t* w = calloc(1, sizeof(trino_wav_t));
    if (!w) {
        fclose(f);
        return NULL;
    }
    w->adpcm = h[4];
    w->bus = h[5];
    w->looped = h[6] & 1;
    w->sample_rate = rd32(h + 8);
    w->nsamples = rd32(h + 12);
    w->data_bytes = rd32(h + 16);
    w->chunk_samples = rd32(h + 20);
    w->ncontexts = rd32(h + 24);
    for (int i = 0; i < 16; i++) w->coefs[i] = (u16)rd16(h + 28 + i * 2);
    // Streams play in whole ADPCM frames (14 samples) per chunk.
    w->stream = (h[6] & 2) && w->chunk_samples && w->nsamples &&
                (!w->adpcm || (w->chunk_samples % 14 == 0 &&
                               w->ncontexts == (w->nsamples + w->chunk_samples - 1) / w->chunk_samples));
    int ok = 1;
    if (w->ncontexts) {
        w->contexts = malloc(w->ncontexts * sizeof(ndspAdpcmData));
        ok = w->contexts != NULL;
        for (uint32_t i = 0; ok && i < w->ncontexts; i++) {
            uint8_t c[8];
            ok = fread(c, 1, 8, f) == 8;
            w->contexts[i].index = (u16)rd16(c);
            w->contexts[i].history0 = rd16(c + 2);
            w->contexts[i].history1 = rd16(c + 4);
        }
        if (ok) s_resident += w->ncontexts * sizeof(ndspAdpcmData);
    }
    w->data_offset = T3AU_HEADER + w->ncontexts * 8;
    if (ok && w->stream) {
        strcpy(w->path, romfs_path);
        if (w->adpcm) w->start = w->contexts[0];
    } else if (ok && w->data_bytes) {
        w->data = linearAlloc(w->data_bytes);
        ok = w->data && fread(w->data, 1, w->data_bytes, f) == w->data_bytes;
        if (ok) {
            DSP_FlushDataCache(w->data, w->data_bytes);
            s_resident += w->data_bytes;
            // Predictor/scale of the first frame, no history: how the
            // encoder started (and how a loop restarts).
            if (w->adpcm) w->start.index = ((uint8_t*)w->data)[0];
        }
    }
    fclose(f);
    if (!ok) {
        if (w->data) linearFree(w->data);
        free(w->contexts);
        free(w);
        return NULL;
    }
    return w;
}

// Bus and flags of a loaded sound: bits 0..8 the bus (trino_core::Bus),
// bit 8 streamed, bit 9 DSP-ADPCM, bit 10 baked to loop.
uint32_t trino_wav_info(void* wav_ptr)
{
    trino_wav_t* w = wav_ptr;
    if (!w) return 0;
    return w->bus | (uint32_t)w->stream << 8 | (uint32_t)w->adpcm << 9 | (uint32_t)w->looped << 10;
}

void trino_channel_stop(uint32_t channel);

// Free a loaded sound (an unloaded group's): the channels playing it stop
// first (a stream's slot under the audio lock), then its memory goes.
void trino_wav_free(void* wav_ptr)
{
    trino_wav_t* w = wav_ptr;
    if (!w) return;
    for (uint32_t ch = 0; ch < TRINO_NDSP_CHANNELS; ch++) {
        LightLock_Lock(&s_audio_lock);
        int slot = s_stream_of[ch];
        int streams = slot >= 0 && s_streams[slot].active && s_streams[slot].wav == w;
        LightLock_Unlock(&s_audio_lock);
        int plays = w->data && s_wavebufs[ch].data_vaddr == w->data;
        if (streams || plays) {
            trino_channel_stop(ch);
            if (plays) s_wavebufs[ch].data_vaddr = NULL;
        }
    }
    if (w->contexts) s_resident -= w->ncontexts * sizeof(ndspAdpcmData);
    if (w->data) {
        s_resident -= w->data_bytes;
        linearFree(w->data);
    }
    free(w->contexts);
    free(w);
}

// Read chunk `st->next_chunk` into buffer `i` and queue it. 0 when there is
// nothing left (non-looping) or the read failed. Holds s_audio_lock.
static int stream_queue(trino_stream_t* st, int i)
{
    trino_wav_t* w = st->wav;
    if (st->next_chunk >= st->nchunks) {
        if (!st->looped) {
            st->ended = 1;
            return 0;
        }
        st->next_chunk = 0;
    }
    uint32_t k = st->next_chunk++;
    uint32_t first = k * w->chunk_samples;
    uint32_t n = w->nsamples - first;
    if (n > w->chunk_samples) n = w->chunk_samples;
    uint32_t off = w->adpcm ? k * (w->chunk_samples / 14) * 8 : first * 2;
    uint32_t bytes = w->adpcm ? (n + 13) / 14 * 8 : n * 2;
    if (bytes > st->buf_bytes || fseek(st->f, (long)(w->data_offset + off), SEEK_SET) != 0 ||
        fread(st->buf[i], 1, bytes, st->f) != bytes) {
        st->ended = 1;
        return 0;
    }
    DSP_FlushDataCache(st->buf[i], bytes);
    ndspWaveBuf* wb = &st->wb[i];
    memset(wb, 0, sizeof(*wb));
    wb->data_vaddr = st->buf[i];
    wb->nsamples = n;
    if (w->adpcm) {
        st->ctx[i] = w->contexts[k];
        wb->adpcm_data = &st->ctx[i];
    }
    ndspChnWaveBufAdd((int)st->channel, wb);
    if (!st->looped && st->next_chunk >= st->nchunks) st->ended = 1;
    return 1;
}

// Buffers the DSP is on (or about to be).
static int stream_pending(const trino_stream_t* st)
{
    int n = 0;
    for (int i = 0; i < TRINO_STREAM_BUFS; i++) {
        u8 status = st->wb[i].status;
        n += status == NDSP_WBUF_QUEUED || status == NDSP_WBUF_PLAYING;
    }
    return n;
}

// Count the buffers the DSP finished (they become free). Holds the lock.
static void stream_collect(trino_stream_t* st)
{
    for (int i = 0; i < TRINO_STREAM_BUFS; i++) {
        if (st->wb[i].status == NDSP_WBUF_DONE) {
            s_streamed += st->wb[i].nsamples;
            st->wb[i].status = NDSP_WBUF_FREE;
        }
    }
}

// Release a playback's buffers and file. Holds the lock.
static void stream_stop(int slot)
{
    trino_stream_t* st = &s_streams[slot];
    // What played of the current buffer still counts as delivered.
    if (stream_pending(st)) s_streamed += ndspChnGetSamplePos((int)st->channel);
    ndspChnWaveBufClear((int)st->channel);
    for (int i = 0; i < TRINO_STREAM_BUFS; i++) {
        if (st->buf[i]) linearFree(st->buf[i]);
        st->buf[i] = NULL;
        st->wb[i].status = NDSP_WBUF_FREE;
    }
    if (st->f) fclose(st->f);
    st->f = NULL;
    s_stream_of[st->channel] = -1;
    st->active = 0;
}

// Start streaming `w` on `channel` (already reset and formatted): open the
// file, allocate the buffers and fill them here, so the first audio frame
// never finds them empty.
static void stream_start(trino_wav_t* w, uint32_t channel, uint32_t looped)
{
    LightLock_Lock(&s_audio_lock);
    int slot = -1;
    for (int i = 0; i < TRINO_MAX_STREAMS && slot < 0; i++)
        if (!s_streams[i].active) slot = i;
    if (slot < 0) {
        LightLock_Unlock(&s_audio_lock);
        return;
    }
    trino_stream_t* st = &s_streams[slot];
    memset(st, 0, sizeof(*st));
    st->wav = w;
    st->channel = channel;
    st->looped = looped != 0;
    st->nchunks = (w->nsamples + w->chunk_samples - 1) / w->chunk_samples;
    st->buf_bytes = w->adpcm ? w->chunk_samples / 14 * 8 : w->chunk_samples * 2;
    st->f = fopen(w->path, "rb");
    int ok = st->f != NULL;
    for (int i = 0; ok && i < TRINO_STREAM_BUFS; i++) {
        st->buf[i] = linearAlloc(st->buf_bytes);
        ok = st->buf[i] != NULL;
    }
    st->active = 1;
    s_stream_of[channel] = slot;
    if (!ok) {
        stream_stop(slot);
    } else {
        for (int i = 0; i < TRINO_STREAM_BUFS && stream_queue(st, i); i++) {
        }
    }
    LightLock_Unlock(&s_audio_lock);
}

// The audio thread: after every ndsp frame, count what played, note a
// stream that ran dry (an underrun: nothing queued and more to come) and
// refill the free buffers.
static void trino_audio_thread(void* arg)
{
    (void)arg;
    while (!s_audio_quit) {
        LightEvent_Wait(&s_audio_wake);
        LightLock_Lock(&s_audio_lock);
        for (int s = 0; s < TRINO_MAX_STREAMS; s++) {
            trino_stream_t* st = &s_streams[s];
            if (!st->active || st->ended) continue;
            stream_collect(st);
            if (stream_pending(st) == 0) s_underruns++;
            for (int i = 0; i < TRINO_STREAM_BUFS && !st->ended; i++)
                if (st->wb[i].status == NDSP_WBUF_FREE) stream_queue(st, i);
        }
        LightLock_Unlock(&s_audio_lock);
    }
}

static void trino_ndsp_frame(void* data)
{
    (void)data;
    LightEvent_Signal(&s_audio_wake);
}

// ndsp needs the DSP firmware: `sdmc:/3ds/dspfirm.cdc` (dumped from the
// console) or the Homebrew Launcher's `hb:ndsp`. Without it ndspInit fails
// and every ndsp call does nothing — sounds sit queued forever. Azahar and
// Citra emulate the DSP at a high level and never run the firmware, so there
// (svcGetSystemInfo 0x20000 answers only on them) a placeholder image is
// enough, and the self-test hears what the console would play.
static u8 s_dsp_placeholder[0x400];

static void trino_ndsp_init(void)
{
    Result rc = ndspInit();
    s64 emulator = 0;
    if (R_FAILED(rc) && R_SUCCEEDED(svcGetSystemInfo(&emulator, 0x20000, 0)) && emulator) {
        ndspUseComponent(s_dsp_placeholder, sizeof(s_dsp_placeholder), 0xFF, 0xFF);
        rc = ndspInit();
    }
    if (R_FAILED(rc)) {
        char msg[96];
        snprintf(msg, sizeof(msg),
                 "TRINO_AUDIO: ndsp unavailable (%08lx): no sdmc:/3ds/dspfirm.cdc\n",
                 (unsigned long)rc);
        trino_log(msg);
    }
}

// The DSP, the stream bookkeeping and the audio thread. The thread takes the
// second core the worker uses (#54: core 2 on a New 3DS, core 1 at 30% on an
// old one; trino_second_core_thread) one priority above the main thread, so
// a refill preempts a long game job instead of queueing behind it; it sleeps
// between ndsp frames. It is a thread of its own, not a worker job: ndsp's
// callback wakes it every audio frame, and a job submitted from there would
// wait in the ring, or run inline in ndsp's thread when the ring is full.
// No second core: the app core, same priority.
static void trino_audio_init(void)
{
    trino_ndsp_init();
    LightLock_Init(&s_audio_lock);
    LightEvent_Init(&s_audio_wake, RESET_ONESHOT);
    for (int i = 0; i < TRINO_NDSP_CHANNELS; i++) s_stream_of[i] = -1;
    s32 prio = 0x30;
    svcGetThreadPriority(&prio, CUR_THREAD_HANDLE);
    if (prio > 0x18) prio--;
    bool n3ds = false;
    APT_CheckNew3DS(&n3ds);
    int32_t core = -1;
    s_audio_thread = trino_second_core_thread(trino_audio_thread, 16 * 1024, prio, n3ds, &core);
    s_audio_core = core;
    if (!s_audio_thread) {
        s_audio_thread = threadCreate(trino_audio_thread, NULL, 16 * 1024, prio, -2, false);
        s_audio_core = -2;
    }
    ndspSetCallback(trino_ndsp_frame, NULL);
}

static void trino_audio_exit(void)
{
    ndspSetCallback(NULL, NULL);
    if (s_audio_thread) {
        s_audio_quit = 1;
        LightEvent_Signal(&s_audio_wake);
        threadJoin(s_audio_thread, U64_MAX);
        threadFree(s_audio_thread);
        s_audio_thread = NULL;
    }
    LightLock_Lock(&s_audio_lock);
    for (int s = 0; s < TRINO_MAX_STREAMS; s++)
        if (s_streams[s].active) stream_stop(s);
    LightLock_Unlock(&s_audio_lock);
}

void trino_channel_stop(uint32_t channel)
{
    if (channel >= TRINO_NDSP_CHANNELS) return;
    LightLock_Lock(&s_audio_lock);
    if (s_stream_of[channel] >= 0) stream_stop(s_stream_of[channel]);
    LightLock_Unlock(&s_audio_lock);
    ndspChnWaveBufClear((int)channel);
    s_wavebufs[channel].status = NDSP_WBUF_DONE;
}

void trino_wav_play(void* wav_ptr, uint32_t channel, uint32_t looped)
{
    trino_wav_t* w = wav_ptr;
    if (!w || channel >= TRINO_NDSP_CHANNELS) return;
    trino_channel_stop(channel);
    ndspChnReset((int)channel);
    ndspChnSetInterp((int)channel, NDSP_INTERP_LINEAR);
    ndspChnSetRate((int)channel, (float)w->sample_rate);
    ndspChnSetFormat((int)channel, w->adpcm ? NDSP_FORMAT_MONO_ADPCM : NDSP_FORMAT_MONO_PCM16);
    if (w->adpcm) ndspChnSetAdpcmCoefs((int)channel, w->coefs);
    if (w->stream) {
        stream_start(w, channel, looped);
        return;
    }
    if (!w->data) return;
    ndspWaveBuf* buf = &s_wavebufs[channel];
    memset(buf, 0, sizeof(*buf));
    buf->data_vaddr = w->data;
    buf->nsamples = w->nsamples;
    buf->looping = looped != 0;
    // Every pass of a loop restarts from the first frame's context.
    if (w->adpcm) {
        s_wavectx[channel] = w->start;
        buf->adpcm_data = &s_wavectx[channel];
    }
    ndspChnWaveBufAdd((int)channel, buf);
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
    ndspChnSetMix((int)channel, mix);
}

// Playing = our wave buffer is queued or playing. Not ndspChnIsPlaying: it
// lags a frame after a play and stays true once the buffer ended (the DSP
// keeps the channel enabled), so a one-shot would never finish. A stream
// plays until its last chunk has been played.
int32_t trino_ch_playing(uint32_t channel)
{
    if (channel >= TRINO_NDSP_CHANNELS) return 0;
    // No lock: only this (the main) thread starts and stops streams; what
    // the audio thread changes meanwhile is at worst a frame stale.
    int slot = s_stream_of[channel];
    if (slot >= 0) {
        trino_stream_t* st = &s_streams[slot];
        return !st->ended || stream_pending(st) > 0;
    }
    u8 st = s_wavebufs[channel].status;
    return (st == NDSP_WBUF_QUEUED || st == NDSP_WBUF_PLAYING) ? 1 : 0;
}

// Once per frame: release the streams that played to their end (next frame
// if the audio thread is reading a chunk right now).
void trino_audio_poll(void)
{
    if (LightLock_TryLock(&s_audio_lock) != 0) return;
    for (int s = 0; s < TRINO_MAX_STREAMS; s++) {
        trino_stream_t* st = &s_streams[s];
        if (st->active && st->ended && stream_pending(st) == 0) {
            stream_collect(st);
            stream_stop(s);
        }
    }
    LightLock_Unlock(&s_audio_lock);
}

// Audio counters (trino_core::AudioStats): 0 streams playing, 1 bytes held
// by loaded sounds (linear data + contexts), 2 bytes of streaming buffers,
// 3 underruns, 4 samples the streams delivered, 5 the audio thread's core
// (2 or 1, the second core the worker uses; 0 = the app core).
uint32_t trino_audio_stat(uint32_t which)
{
    // Lock-free like trino_ch_playing.
    uint32_t v = 0;
    switch (which) {
    case 0:
    case 2:
    case 4:
        if (which == 4) v = s_streamed;
        for (int s = 0; s < TRINO_MAX_STREAMS; s++) {
            trino_stream_t* st = &s_streams[s];
            if (!st->active) continue;
            if (which == 0) v += !st->ended || stream_pending(st) > 0;
            if (which == 2) v += TRINO_STREAM_BUFS * st->buf_bytes;
            if (which == 4 && stream_pending(st)) v += ndspChnGetSamplePos((int)st->channel);
        }
        break;
    case 1: v = s_resident; break;
    case 3: v = s_underruns; break;
    case 5: v = s_audio_core > 0 ? (uint32_t)s_audio_core : 0; break;
    }
    return v;
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

// Ranged reads of one RomFS file (an asset group's bundle: its table, then
// each entry, never the whole file at once).
void* trino_file_open(const char* romfs_path)
{
    return fopen(romfs_path, "rb");
}

// `len` bytes at `offset` into `dest`; the bytes read (short = error/EOF).
uint32_t trino_file_read_at(void* file, uint32_t offset, void* dest, uint32_t len)
{
    FILE* f = (FILE*)file;
    if (!f || fseek(f, (long)offset, SEEK_SET) != 0) return 0;
    return (uint32_t)fread(dest, 1, len, f);
}

void trino_file_close(void* file)
{
    if (file) fclose((FILE*)file);
}

extern u32 __ctru_heap_size;
extern u32 __ctru_linear_heap_size;

// The memory assets live in, for Platform::memory: which 0 = bytes in use
// (newlib's malloc plus the linear heap: textures, meshes, sounds), 1 = both
// heaps' size.
uint32_t trino_heap_stat(uint32_t which)
{
    if (which == 1) return __ctru_heap_size + __ctru_linear_heap_size;
    struct mallinfo mi = mallinfo();
    return (uint32_t)mi.uordblks + (__ctru_linear_heap_size - linearSpaceFree());
}

// ---------------------------------------------------------------------------
// Entry: the 3dsx crt0 calls main(); Rust takes over immediately.

void trino_rust_main(void);

int main(void)
{
    trino_rust_main();
    // Rust returns when aptMainLoop() says stop: tear down in reverse.
    trino_audio_exit();
    trino_worker_stop();
    ndspExit();
    C2D_Fini();
    C3D_Fini();
    romfsExit();
    gfxExit();
    return 0;
}
