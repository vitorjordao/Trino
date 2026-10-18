##############################################################################
# rsp_trino.S — Trino's RSP overlay (rspq): vertex T&L for static meshes.
#
# The CPU path (trino_core::tessellate) stays the reference: the N64
# renderer sends a draw here only when trino_core::render3d::rsp_eligible
# says the result needs nothing this overlay lacks (no clipping: the whole
# bounding sphere is inside the guard band; white light, no fog, opaque,
# untextured, static). The overlay transforms, lights and projects each
# vertex in fixed point, then hands every triangle to libdragon's
# RDPQ_Triangle_Send_Async (rsp_rdpq_tri.inc), the same RDP triangle setup
# the CPU path's rdpq_triangle runs.
#
# Assembled twice (xtask `compile_shim`): as is, rsp_trino, with one light
# intensity (ambient + (1 - ambient) * n.l); with TRINO_COLORED defined,
# rsp_trino_color, for a colored or dimmed light, an emissive draw or fog:
# per-channel ambient and diffuse terms, an emissive add and a per-vertex
# fog blend (the fog factor is a fifth row of the clip matrix, lane 4).
# With TRINO_TEXTURED as well, rsp_trino_tex: textured draws (S, T, W and
# 1/W per vertex for the RDP's perspective-correct textured triangles).
# A second overlay, not a second command: rspq reloads an overlay's code on every
# switch to it, and the colored loop in the same overlay made every
# white-light frame pay for the bigger code (measured in ares, 2026-10-16).
#
# Mesh data (built once by the CPU at register, see the N64 renderer) is a
# table of segments of at most SEG_VERTS vertices; each segment is one
# DMA: its 32-byte vertices, then 3 local u8 indices per triangle.
#
#   vertex  +0  x, y, z, 1   s15.16 integer parts
#           +8  x, y, z, 0   s15.16 fractional parts
#           +16 nx, ny, nz, 0  s16, s0.15
#           +24 r, g, b, a, 0, 0, 0, 0  u8
#           +32 u, v, 0, 0  s5.10 (TRINO_TEXTURED only: 40-byte vertices)
##############################################################################

#include <rsp_queue.inc>
#include <rdpq_macros.h>

    .set noreorder
    .set at

#define SEG_VERTS       32
#ifdef TRINO_TEXTURED
#define VTX_IN_SIZE     40      // + u, v (s5.10, repeats), 4 pad bytes
#else
#define VTX_IN_SIZE     32
#endif
#define VTX_OUT_SIZE    24
#if defined(TRINO_WATER)
#define DRAW_CMD_SIZE   0xB8    // + the water block and ripple buffer, see DRAW
#elif defined(TRINO_TEXTURED)
#define DRAW_CMD_SIZE   0xB0    // + the texture scale and offset, see DRAW
#elif defined(TRINO_COLORED)
#define DRAW_CMD_SIZE   0xA0    // + the per-channel terms and the fog, see DRAW
#else
#define DRAW_CMD_SIZE   0x6C
#endif
#define TRI_SLOT        0xB0    // RDPQ_CMD_STAGING's size: the routine writes up to it
#define WBLK_SIZE       0x240   // the water block (rsp::WATER_BLOCK)
#define WBLK_PART       0x180   // its per-frame part's start (rsp::WATER_FRAME_PART)

    .data

    RSPQ_BeginOverlayHeader
        RSPQ_DefineCommand TrinoCmd_TestTri, 4                  # 0x0
        RSPQ_DefineCommand TrinoCmd_Draw,    DRAW_CMD_SIZE      # 0x1
    RSPQ_EndOverlayHeader

    .align 4
#if defined(TRINO_WATER)
BANNER: .ascii "Trino RSP T&L w "
#elif defined(TRINO_TEXTURED)
BANNER: .ascii "Trino RSP T&L t "
#elif defined(TRINO_COLORED)
BANNER: .ascii "Trino RSP T&L c "
#else
BANNER: .ascii "Trino RSP T&L   "
#endif

    RSPQ_EmptySavedState

    # Viewport: clip space x/y in [-w, w] -> s13.2 screen coordinates of a
    # 320x240 frame (160*4, 120*4). Lane 2 of VIEW_SCALE and lane 3 of
    # VIEW_OFFSET give the z-buffer value 0x7FFF * (1 - NEAR / depth)
    # (NEAR = 0.05, NEAR * 0x7FFF = 1638) straight from 1 / depth, as
    # 0x7FFE - floor(1638 / depth): the CPU path truncates
    # 0x7FFF - 1638.35 / depth, and 0x7FFF here sat one unit farther, which
    # flipped the depth test where RSP and CPU geometry touch.
    .align 4
#ifdef TRINO_COLORED
    # Lane 4: 0x4000, the fog factor's s15.16 -> s1.14 scale.
VIEW_SCALE:     .half 160*4, 120*4, 0x7FFE, 0, 0x4000, 0, 0, 0
#else
VIEW_SCALE:     .half 160*4, 120*4, 0x7FFE, 0, 0, 0, 0, 0
#endif
VIEW_OFFSET:    .half 160*4, 120*4, 0, 1638, 0, 0, 0, 0
#ifdef TRINO_COLORED
    # The fog blend changes red, green and blue, never alpha.
    .align 4
FOG_MASK:       .half 0xFFFF, 0xFFFF, 0xFFFF, 0, 0, 0, 0, 0
#endif
#ifdef TRINO_COLORED
    # The paired paths' viewport and fog mask, one copy per half.
    .align 4
VIEW_SCALE2:    .half 160*4, 120*4, 0x7FFE, 0, 160*4, 120*4, 0x7FFE, 0
VIEW_OFFSET2:   .half 160*4, 120*4, 0, 1638, 160*4, 120*4, 0, 1638
FOG_MASK2:      .half 0xFFFF, 0xFFFF, 0xFFFF, 0, 0xFFFF, 0xFFFF, 0xFFFF, 0
#endif
#ifdef TRINO_WATER
    # The water's constants: unused, 1 - epsilon, sin_turns' P (0.225),
    # 1/2, the Fresnel term's 0.85 * 0.98 and 0.85 * 0.02, unused, 1/4.
    .align 4
WCONST:         .half 0x8000, 0x7FFF, 7373, 0x4000, 27296, 557, 0xFFFF, 0x2000
    # The ripples' constants (water::ripple_sample, RIPPLE_WIDTH W = 0.7,
    # RIPPLE_WAVELENGTH L = 0.55): 64 / W as s15.16 (integer, fraction),
    # 1 / (128 L) as s15.16 (integer, fraction), -4 / W / 8 and
    # 2 pi / L / 16 (s0.15), unused.
    .align 4
RCONST:         .half 91, 28087, 0, 931, -23406, 23396, 0, 0
#endif

#ifndef TRINO_WATER
    # Milestone 1 check: three screen-space vertices in rsp_rdpq_tri.inc's
    # default layout.
    .align 3
TEST_VTX:
    .half 40*4, 40*4,   0x4000
    .byte 0, 0x3F
    .word 0xFF0000FF, 0, 0, 0
    .align 3
    .half 80*4, 40*4,   0x4000
    .byte 0, 0x3F
    .word 0x00FF00FF, 0, 0, 0
    .align 3
    .half 40*4, 80*4,   0x4000
    .byte 0, 0x3F
    .word 0x0000FFFF, 0, 0, 0
#endif

    .bss
    # The draw command, copied to an aligned place for vector loads:
    #   +0x00 cmd | cull (0 = cull front, 1 = cull back, 2 = none)
    #   +0x04 segment table (RDRAM, walked an entry per segment)
    #   +0x08 segment count | triangle command
    #   +0x0C unused                  +0x10 matrix columns, integer parts
    #   +0x30 matrix columns, fractional parts
    #   +0x50 light (s0.15, model space, toward the light) x, y, z, 0
    #   +0x58 tint r, g, b, 0 (s0.15)  +0x60 ambient, 1 - ambient, tint a, 0
    #   +0x68 debug: RDRAM address for segment 0's screen vertices (0 = off)
#ifdef TRINO_COLORED
    #   +0x6C unused
    #   +0x70 ambient r, g, b, 0   +0x78 diffuse r, g, b, 0 (s0.15)
    #   +0x80 emissive r, g, b, 0 (u8 << 7, the luv layout)
    #   +0x88 fog row, integer parts, one per matrix column (s15.16):
    #         (view depth - fog near) / (fog far - fog near)
    #   +0x90 fog row, fractional parts   +0x98 fog r, g, b, 0 (u8 << 7)
#ifdef TRINO_TEXTURED
    #   +0xA0 texels per u, v repeat / 32, integer parts (s, t)
    #   +0xA4 the same, fractional parts (s15.16: S = u * scale, s10.5)
    #   +0xA8 S, T offset (s10.5)       +0xAC unused
#endif
#ifdef TRINO_WATER
    #   +0xB0 the water block (RDRAM, WBLK below)   +0xB4 unused
#endif
    .align 4
DRAW:           .ds.b DRAW_CMD_SIZE
#else
    .align 4
DRAW:           .ds.b 0x70
#endif
    .align 4
SEG_TABLE:      .ds.b 16           // the current segment's entry
    .align 4
VTX_IN:         .ds.b SEG_VERTS * VTX_IN_SIZE + 3 * 64
    .align 4
VTX_OUT:        .ds.b SEG_VERTS * VTX_OUT_SIZE
    # Staging for the RDP triangle command. One buffer, reused for every
    # triangle like rdpq's own RDPQ_CMD_STAGING: RDPQ_Triangle_Send_Async
    # waits for the previous triangle's DMA itself. (Alternating two buffers
    # dropped every other pair of triangles.)
    .align 4
TRI_STAGING:    .ds.b TRI_SLOT
#ifdef TRINO_WATER
    # The water block (the CPU's `rsp::water_block`). Rows of 8 lanes, the
    # second half a copy of the first (two vertices per pass):
    #   +0x000 world x, z rows of matrix column c (lanes 0, 1) at c * 0x20,
    #          integer parts; +0x10 the fractional parts
    #   +0x080 UP: clip x, y, z, w per world unit of height (int), +0x090 frac
    #   +0x0A0 AX, +0x0C0 BZ (int, then frac rows): the swell's phase terms
    #          in x, z, lanes sin0, cos0, sin1, cos1
    #   +0x0E0 AMP, +0x0F0 GX, +0x100 GZ: height and half gradient per sine
    #   +0x110 8192 / fx, -8192 / fy (a, b in s2.13 from the screen math),
    #          glint on, glint ^16 (lanes 2, 3); the ring flag in lane 6: a
    #          ring draw (rsp::ring_segment) keeps its vertex color, unlit
    #   +0x120 RC0, RC1, RC2: the view's rotation columns; +0x150 sun (view)
    #   +0x160 sky r, g, b, 255 (u8 << 7); +0x170 glint color (u8 << 7)
    # The frame's part (WBLK_PART on, a second DMA: shared by every draw of
    # one water material in a frame):
    #   +0x180 C (int, frac rows): the swell's phases
    #   +0x1A0 the ripples, one per lane 0..3 (zero beyond the last): x and
    #          z (s15.16: integer rows, fractional rows), 128 * radius
    #          (s15.16), strength * response * decay (s0.15), and each band's
    #          d^2 bounds (integer parts, rounded outward: outer + 1, inner)
    #   +0x230 the ripple count (0 skips their sum)
    .align 4
WBLK:           .ds.b WBLK_SIZE
    .align 4
WSCR:           .ds.b 0x50         // per pass: nv, (a, b), -, n . l, S T (A, B)
#endif

    .text

    .func TrinoCmd_TestTri
TrinoCmd_TestTri:
#ifdef TRINO_WATER
    # No test triangle here: IMEM is full.
    j RSPQ_Loop
    nop
#else
    andi a0, 0xFFFF
    li s3, %lo(TRI_STAGING)
    li v0, 2                        # no culling
    li a1, %lo(TEST_VTX) + 0
    li a2, %lo(TEST_VTX) + 24
    jal RDPQ_Triangle_Send_Async
    li a3, %lo(TEST_VTX) + 48
    j trino_test_done
    nop
#endif
    .endfunc

    #############################################################
    # TrinoCmd_Draw: T&L + rasterize one static mesh draw.
    #############################################################
    .func TrinoCmd_Draw
TrinoCmd_Draw:
    #define seg_ptr   s5
    #define seg_left  s6
    #define src       s1
    #define dst       s2
    #define count     s7
    #define idx       fp

    # Copy the command (DRAW_CMD_SIZE bytes) into DRAW.
    addiu t0, rspq_dmem_buf_ptr, %lo(RSPQ_DMEM_BUFFER) - DRAW_CMD_SIZE
    li t1, %lo(DRAW)
    li t2, DRAW_CMD_SIZE
1:  lw t3, 0(t0)
    addiu t0, 4
    addiu t2, -4
    sw t3, 0(t1)
    bgtz t2, 1b
    addiu t1, 4
#ifdef TRINO_WATER
    # The draw's water block (WBLK): its static part (the draw's own,
    # remembered by the CPU), then the frame's (the swell's phases and the
    # ripples, shared by every draw of the material).
    lw s0, %lo(DRAW) + 0xB0
    li s4, %lo(WBLK)
    jal DMAIn
    li t0, WBLK_PART - 1
    lw s0, %lo(DRAW) + 0xB4
    li s4, %lo(WBLK) + WBLK_PART
    jal DMAIn
    li t0, WBLK_SIZE - WBLK_PART - 1
#endif

    # The segment table stays in RDRAM: each segment's 16-byte entry is
    # fetched on its turn (DRAW +0x04 walks the table), so a mesh may have
    # any number of segments.
    lhu seg_left, %lo(DRAW) + 0x08
    li seg_ptr, %lo(SEG_TABLE)

draw_segment:
    lw s0, %lo(DRAW) + 0x04
    addiu t0, s0, 16
    sw t0, %lo(DRAW) + 0x04
    li s4, %lo(SEG_TABLE)
    jal DMAIn
    li t0, 15

    # One DMA: the segment's vertices then its indices.
    lw s0, 0(seg_ptr)
    lhu t0, 4(seg_ptr)              # bytes
    li s4, %lo(VTX_IN)
    jal DMAIn
    addiu t0, -1

    #define vm0i  $v16
    #define vm1i  $v17
    #define vm2i  $v18
    #define vm3i  $v19
    #define vm0f  $v20
    #define vm1f  $v21
    #define vm2f  $v22
    #define vm3f  $v23
    #define vposf $v24
    #define vlight $v25
    #define vtint $v26
    #define vamb  $v27      // ambient, 1 - ambient, tint a, 0
    #define vvs   $v28
    #define vvo   $v29
    #define vpos  $v01
    #define vo_i  $v02
    #define vo_f  $v03
    #define vinv_i $v04
    #define vinv_f $v05
    #define vs_i  $v06
    #define vs_f  $v07
    #define vn    $v08
    #define vp    $v09
    #define vd    $v10
    #define vk    $v11
    #define vkt   $v12
    #define vcol  $v13
    #define vtmp  $v14
    #define vta   $v15
    // The paired path (colored, not water): the position's fraction, then
    // the light products; the color terms, kept in both halves.
    #define pa_posf $v09
    #define pa_diff $v24
    #define pa_ambc $v27
    #define pa_emis $v08
#ifdef TRINO_COLORED
    // The per-channel light terms, reloaded per vertex from DRAW into
    // registers the screen math is done with.
    #define vdiff $v02
    #define vambc $v03
    #define vemis $v04
    #define dptr  t5
#endif

#ifdef TRINO_COLORED
    # Both halves (two vertices per pass, see below).
    li t0, %lo(DRAW)
    ldv vm0i.e0, 0x10, t0
    ldv vm0i.e4, 0x10, t0
    ldv vm1i.e0, 0x18, t0
    ldv vm1i.e4, 0x18, t0
    ldv vm2i.e0, 0x20, t0
    ldv vm2i.e4, 0x20, t0
    ldv vm3i.e0, 0x28, t0
    ldv vm3i.e4, 0x28, t0
    ldv vm0f.e0, 0x30, t0
    ldv vm0f.e4, 0x30, t0
    ldv vm1f.e0, 0x38, t0
    ldv vm1f.e4, 0x38, t0
    ldv vm2f.e0, 0x40, t0
    ldv vm2f.e4, 0x40, t0
    ldv vm3f.e0, 0x48, t0
    ldv vm3f.e4, 0x48, t0
    ldv vlight.e0, 0x50, t0
    ldv vtint.e0, 0x58, t0
    ldv vtint.e4, 0x58, t0
#ifndef TRINO_WATER
    ldv vlight.e4, 0x50, t0
    ldv pa_ambc.e0, 0x70, t0
    ldv pa_ambc.e4, 0x70, t0
    ldv pa_diff.e0, 0x78, t0
    ldv pa_diff.e4, 0x78, t0
    ldv pa_emis.e0, 0x80, t0
    ldv pa_emis.e4, 0x80, t0
    li t1, %lo(VIEW_SCALE2)         # (water loads them per pass)
    lqv vvs, 0, t1
    lqv vvo, 0x10, t1
#endif
    # Tint alpha alone in lanes 3 and 7.
    vxor vta, vta, vta
    lsv vta.e3, 0x64, t0
    lsv vta.e7, 0x64, t0
#else
    li t0, %lo(DRAW)
    ldv vm0i.e0, 0x10, t0
    ldv vm1i.e0, 0x18, t0
    ldv vm2i.e0, 0x20, t0
    ldv vm3i.e0, 0x28, t0
    ldv vm0f.e0, 0x30, t0
    ldv vm1f.e0, 0x38, t0
    ldv vm2f.e0, 0x40, t0
    ldv vm3f.e0, 0x48, t0
    ldv vlight.e0, 0x50, t0
    ldv vtint.e0, 0x58, t0
    ldv vamb.e0, 0x60, t0
    li t1, %lo(VIEW_SCALE)
    lqv vvs, 0, t1
    lqv vvo, 0x10, t1
    # Tint alpha alone in lane 3: kt = tint_rgb * k + (0, 0, 0, tint_a).
    vxor vta, vta, vta
    vmov vta.e3, vamb.e2
#endif

#ifdef TRINO_WATER
    #define wptr t6
    #define wscr t8
    li wptr, %lo(WBLK)
    li wscr, %lo(WSCR)
#endif
    lhu count, 6(seg_ptr)
    li src, %lo(VTX_IN)
    li dst, %lo(VTX_OUT)
#ifdef TRINO_COLORED
    li dptr, %lo(DRAW)
#endif

transform_vertex:
#ifdef TRINO_WATER
    # ------------------------------------------------------------------
    # Water vertices (ADR 0015, issue #69): the CPU path's WATER copy of
    # the T&L in fixed point, two vertices per pass like the colored path
    # below (A in lanes 0..3, B in lanes 4..7; the block's rows hold both
    # halves). Displaced along world Y by the swell (two sines) and the
    # ripples, lit from the displaced normal, the Fresnel sky and the sun's
    # glint added, fogged and textured. The vector unit waits on every
    # dependent instruction, so one vertex per pass left it idle most of
    # the time (~17 us a vertex in ares).
    # ------------------------------------------------------------------
    #define wv1   $v01
    #define wv2   $v04
    #define wv3   $v05
    #define wv4   $v06
    #define wv5   $v08
    #define wv6   $v09
    #define wv7   $v10
    #define wv8   $v11
    #define wv9   $v12
    #define wvc   $v13      // WCONST
    #define wo_i  $v24      // world x, z (lanes 0, 1), then RCONST
    #define wo_f  $v27
    #define gh    $v08      // the height (lanes 0, 4)
    #define gx    $v28      // half its x and z gradient (lanes 1, 5);
    #define gz    $v29      // vvs, vvo are loaded again for the screen
    #define wcol  $v27      // the color (wo_f's, after the ripples)
    #define wk    $v06
    ldv vpos.e0, 0, src
    ldv vpos.e4, VTX_IN_SIZE, src
    ldv wv6.e0, 8, src              # the positions' fractions
    ldv wv6.e4, VTX_IN_SIZE + 8, src
    # The world x, z rows of each column (integer, fractional parts).
    lqv $v04, 0x00, wptr
    lqv $v05, 0x10, wptr
    lqv $v06, 0x20, wptr
    lqv $v07, 0x30, wptr
    lqv $v10, 0x40, wptr
    lqv $v11, 0x50, wptr
    lqv $v12, 0x60, wptr
    lqv $v13, 0x70, wptr
    # Clip x, y, z, w.
    vmudl vtmp, vm0f, wv6.h0
    vmadm vtmp, vm0i, wv6.h0
    vmadn vtmp, vm0f, vpos.h0
    vmadh vtmp, vm0i, vpos.h0
    vmadl vtmp, vm1f, wv6.h1
    vmadm vtmp, vm1i, wv6.h1
    vmadn vtmp, vm1f, vpos.h1
    vmadh vtmp, vm1i, vpos.h1
    vmadl vtmp, vm2f, wv6.h2
    vmadm vtmp, vm2i, wv6.h2
    vmadn vtmp, vm2f, vpos.h2
    vmadh vtmp, vm2i, vpos.h2
    vmadn vo_f, vm3f, vpos.h3
    vmadh vo_i, vm3i, vpos.h3
    # World x, z.
    vmudl vtmp, $v05, wv6.h0
    vmadm vtmp, $v04, wv6.h0
    vmadn vtmp, $v05, vpos.h0
    vmadh vtmp, $v04, vpos.h0
    vmadl vtmp, $v07, wv6.h1
    vmadm vtmp, $v06, wv6.h1
    vmadn vtmp, $v07, vpos.h1
    vmadh vtmp, $v06, vpos.h1
    vmadl vtmp, $v11, wv6.h2
    vmadm vtmp, $v10, wv6.h2
    vmadn vtmp, $v11, vpos.h2
    vmadh vtmp, $v10, vpos.h2
    vmadn wo_f, $v13, vpos.h3
    vmadh wo_i, $v12, vpos.h3

    # The swell's phases in turns (s15.16): T = AX * x + BZ * z + C, lanes
    # sin0, cos0 (+ 1/4 turn), sin1, cos1. Only the fraction matters.
    li t0, %lo(WCONST)
    lqv wvc, 0, t0
    lqv wv2, 0xA0, wptr
    lqv wv3, 0xB0, wptr
    lqv wv4, 0xC0, wptr
    lqv wv5, 0xD0, wptr
    lqv wv6, 0x180, wptr
    lqv wv7, 0x190, wptr
    vmudl vtmp, wv3, wo_f.h0
    vmadm vtmp, wv2, wo_f.h0
    vmadn vtmp, wv3, wo_i.h0
    vmadh vtmp, wv2, wo_i.h0
    vmadl vtmp, wv5, wo_f.h1
    vmadm vtmp, wv4, wo_f.h1
    vmadn vtmp, wv5, wo_i.h1
    vmadh vtmp, wv4, wo_i.h1
    vmadn wv8, wv7, K1
    vmadh vtmp, wv6, K1
    jal water_sin                   # sin0, cos0, sin1, cos1 (s0.15)
    lqv wv2, 0xE0, wptr
    # Height (m, s0.15) and half the gradient: sums of two lanes.
    vmulf wv2, wv8, wv2
    vadd gh, wv2, wv2.h2            # lanes 0, 4: h
    lqv wv3, 0xF0, wptr
    vmulf wv3, wv8, wv3
    vadd gx, wv3, wv3.h3            # lanes 1, 5: dh/dx / 2
    lqv wv4, 0x100, wptr
    vmulf wv4, wv8, wv4
    vadd gz, wv4, wv4.h3            # lanes 1, 5: dh/dz / 2
    # The ripples' (h, gx / 2, gz / 2), water::ripple_sample on lanes 0..3
    # (one ripple each): d from a reciprocal square root, q = (d - r) / W,
    # e = max(0, 1 - q^2) (zero outside the band), the ring's phase
    # t = (d - r) / L through sin_turns; h = s e^2 cos, dh/dd =
    # s (de cos - e^2 (2 pi / L) sin), the gradient dh/dd (dx, dz) / d.
    lhu t0, 0x230(wptr)
    beqz t0, water_ripples_done
    lqv $v01, 0x1A0, wptr           # px - x, 32 bits (-dx)
    lqv $v04, 0x1B0, wptr
    vsubc $v04, $v04, wo_f.h0
    vsub $v01, $v01, wo_i.h0
    lqv $v05, 0x1C0, wptr           # pz - z (-dz)
    lqv $v06, 0x1D0, wptr
    vsubc $v06, $v06, wo_f.h1
    vsub $v05, $v05, wo_i.h1
    li t0, %lo(RCONST)
    lqv wo_i, 0, t0
    vmudl vtmp, $v04, $v04          # d^2 (s15.16)
    vmadm vtmp, $v01, $v04
    vmadn vtmp, $v04, $v01
    vmadh vtmp, $v01, $v01
    vmadl vtmp, $v06, $v06
    vmadm vtmp, $v05, $v06
    vmadn $v09, $v06, $v05
    vmadh $v07, $v05, $v05
    # Inside no ripple's band (d^2's integer part against each band's,
    # rounded outward), in either vertex: nothing to add. Most leave here.
    lqv $v10, 0x210, wptr
    vlt vtmp, $v07, $v10
    cfc2 t0, COP2_CTRL_VCC
    lqv $v10, 0x220, wptr
    vge vtmp, $v07, $v10
    cfc2 t1, COP2_CTRL_VCC
    and t0, t1
    andi t0, 0xFF
    beqz t0, water_ripples_done
    # R = 128 / d (s15.16): vrsq of an s15.16 value, lane by lane.
    vrsqh $v10.e0, $v07.e0
    vrsql $v11.e0, $v09.e0
    vrsqh $v10.e0, vzero.e0
    vrsqh $v10.e1, $v07.e1
    vrsql $v11.e1, $v09.e1
    vrsqh $v10.e1, vzero.e0
    vrsqh $v10.e2, $v07.e2
    vrsql $v11.e2, $v09.e2
    vrsqh $v10.e2, vzero.e0
    vrsqh $v10.e3, $v07.e3
    vrsql $v11.e3, $v09.e3
    vrsqh $v10.e3, vzero.e0
    vrsqh $v10.e4, $v07.e4
    vrsql $v11.e4, $v09.e4
    vrsqh $v10.e4, vzero.e0
    vrsqh $v10.e5, $v07.e5
    vrsql $v11.e5, $v09.e5
    vrsqh $v10.e5, vzero.e0
    vrsqh $v10.e6, $v07.e6
    vrsql $v11.e6, $v09.e6
    vrsqh $v10.e6, vzero.e0
    vrsqh $v10.e7, $v07.e7
    vrsql $v11.e7, $v09.e7
    vrsqh $v10.e7, vzero.e0
    vmudl vtmp, $v09, $v11          # 128 d = d^2 R
    vmadm vtmp, $v07, $v11
    vmadn $v09, $v09, $v10
    vmadh $v07, $v07, $v10
    vmudl vtmp, $v04, $v11          # -dx / d (s0.15): -dx R / 128
    vmadm vtmp, $v01, $v11
    vmadn $v04, $v04, $v10
    vmadh $v01, $v01, $v10
    vmudl vtmp, $v04, K256
    vmadh $v01, $v01, K256
    vmudl vtmp, $v06, $v11          # -dz / d
    vmadm vtmp, $v05, $v11
    vmadn $v06, $v06, $v10
    vmadh $v05, $v05, $v10
    vmudl vtmp, $v06, K256
    vmadh $v05, $v05, K256
    lqv $v10, 0x1E0, wptr           # 128 (d - r)
    lqv $v11, 0x1F0, wptr
    vsubc $v09, $v09, $v11
    vsub $v07, $v07, $v10
    vmudl vtmp, $v09, wo_i.e1       # q (s2.13) = 128 (d - r) * 64 / W / 128
    vmadm vtmp, $v07, wo_i.e1
    vmadn vtmp, $v09, wo_i.e0
    vmadh $v12, $v07, wo_i.e0
    vmudl vtmp, $v09, wo_i.e3       # t's turn fraction: 128 (d - r) / (128 L)
    vmadm vtmp, $v07, wo_i.e3
    vmadn $v09, $v09, wo_i.e2
    vmulf vtmp, $v12, $v12          # e = max(0, 1 - q^2) (s4.11), then s0.15
    vsub vtmp, vzero, vtmp
    vadd vtmp, vtmp, K2048
    vge vtmp, vtmp, vzero
    vmudh vtmp, vtmp, K16
    vmulf $v07, vtmp, $v12          # de = -4 e q / W (s2.13)
    vmulf $v07, $v07, wo_i.e4
    vmudh $v07, $v07, K8
    vmulf $v06, vtmp, vtmp          # e^2
    vaddc $v10, $v09, K16384        # t + 1/4
    vaddc vtmp, vzero, vzero        # clear the carries (vadd adds them)
    jal water_sin                   # sin_turns(t), then into $v04
    vor $v11, vzero, $v09
    vor $v04, vzero, $v11
    jal water_sin                   # cos_turns(t) = sin_turns(t + 1/4)
    vor $v11, vzero, $v10
    lqv vtmp, 0x200, wptr           # s
    vmulf $v10, $v06, vtmp          # h = s e^2 cos
    vmulf $v10, $v10, $v11
    vmulf $v07, $v07, $v11          # de cos (s2.13), then s4.11
    vmulf $v07, $v07, K8192
    vmulf $v12, $v06, $v04          # e^2 sin (2 pi / L) (s4.11)
    vmulf $v12, $v12, wo_i.e5
    vsub $v07, $v07, $v12           # dh/dd (s4.11)
    vmulf $v07, $v07, vtmp
    vmulf $v04, $v01, $v07          # gx / 2 = dh/dd dx / d / 2
    vmudh $v04, $v04, K8
    vsub $v04, vzero, $v04
    vmulf $v06, $v05, $v07          # gz / 2
    vmudh $v06, $v06, K8
    vsub $v06, vzero, $v06
    # The four ripples' sums: h in lane 0, the gradient in lane 1 (where
    # the swell's are).
    vadd $v10, $v10, $v10.q1
    vadd $v10, $v10, $v10.h2
    vadd $v04, $v04, $v04.q0
    vadd $v04, $v04, $v04.h3
    vadd $v06, $v06, $v06.q0
    vadd $v06, $v06, $v06.h3
    vadd gh, gh, $v10
    vadd gx, gx, $v04
    vadd gz, gz, $v06
water_ripples_done:

    # Displace: clip y etc. += h * UP. h as s15.16: fraction 2h, integer
    # -1 below zero.
    vsub wv6, vzero, K1             # -1
    vlt wv7, gh, vzero
    vmrg wv6, wv6, vzero            # h's integer part
    vaddc wv7, gh, gh               # h's fraction
    lqv wv2, 0x80, wptr             # UP, integer parts
    lqv wv3, 0x90, wptr             # UP, fractional parts
    vmudn vtmp, vo_f, K1
    vmadh vtmp, vo_i, K1
    vmadl vtmp, wv3, wv7.h0
    vmadm vtmp, wv2, wv7.h0
    vmadn vo_f, wv3, wv6.h0
    vmadh vo_i, wv2, wv6.h0
    vaddc vtmp, vzero, vzero        # clear the carries (vadd adds them)

    # The normal (-gx, 1, -gz) / sqrt(1 + gx² + gz²). s = (g / 2)², and
    # w = 1 / sqrt(1 + 4s) from w0 = 1 - s by two Newton steps (see
    # water_newton). Then the normal in view space (RC0 nx + RC1 ny + RC2
    # nz) and n . l, kept for the light below. A ring computes them too
    # (unused: one branch less to fit IMEM).
    vmulf wv2, gx, gx
    vmulf wv6, gz, gz
    vadd wv2, wv2, wv6              # lane 1: s
    vadd wv3, wv2, wvc.e7           # lane 1: (1 + 4s) / 4
    vsub wv4, vzero, wv2
    jal water_newton
    vadd wv4, wv4, wvc.e1           # lane 1: w0 = 1 - s
    jal water_newton
    nop                             # lane 1: w
    vmulf wv6, gx, wv4.h1
    vadd wv6, wv6, wv6
    vsub wv6, vzero, wv6            # lane 1: -gx w (n.x)
    vmulf wv7, gz, wv4.h1
    vadd wv7, wv7, wv7
    vsub wv7, vzero, wv7            # lane 1: -gz w (n.z)
    lqv wv2, 0x120, wptr            # RC0, RC1, RC2
    lqv wv3, 0x130, wptr
    lqv wv8, 0x140, wptr
    vmulf wv9, wv2, wv6.h1
    vmacf wv9, wv3, wv4.h1
    vmacf wv9, wv8, wv7.h1          # nv
    sqv wv9, 0x00, wscr
    vmulf wv2, wv6, vlight.e0
    vmulf wv3, wv4, vlight.e1
    vmulf wv8, wv7, vlight.e2
    vadd wv2, wv2, wv3
    vadd wv2, wv2, wv8              # lane 1: n . l
    sqv wv2, 0x30, wscr

    # 1 / w.
    vrcph vinv_i.e3, vo_i.e3
    vrcpl vinv_f.e3, vo_f.e3
    vrcph vinv_i.e3, vzero.e0
    vrcph vinv_i.e7, vo_i.e7
    vrcpl vinv_f.e7, vo_f.e7
    vrcph vinv_i.e7, vzero.e0
    vmudl vtmp, vo_f, vinv_f.h3
    vmadm vtmp, vo_i, vinv_f.h3
    vmadn vs_f, vo_f, vinv_i.h3
    vmadh vs_i, vo_i, vinv_i.h3
    # a, b = view x / depth, view y / depth (s2.13): the view direction.
    lqv wv5, 0x110, wptr
    vmudn vtmp, vs_f, wv5
    vmadh wv5, vs_i, wv5
    sqv wv5, 0x10, wscr
    li t1, %lo(VIEW_SCALE2)
    lqv vvs, 0, t1
    lqv vvo, 0x10, t1
    vmudn vs_f, vs_f, vvs
    vmadh vs_i, vs_i, vvs
    vadd vs_i, vs_i, vvo
    vmudn vtmp, vinv_f, vvo.e3
    vmadh vtmp, vinv_i, vvo.e3
    vsub vtmp, vvs, vtmp.h3
    vmov vs_i.e2, vtmp.e2
    vmov vs_i.e6, vtmp.e6
    sdv vs_i.e0, 0, dst             # X, Y, Z (+ W in the flag bytes)
    sdv vs_i.e4, VTX_OUT_SIZE, dst
    # The fog factor from the view depth (twice w, displaced): f = w FS +
    # FO, s0.15 in [0, 1] (lanes 3, 7).
    ldv vp.e0, 0x88, dptr
    vmudl vtmp, vo_f, vp.e1
    vmadm vtmp, vo_i, vp.e1
    vmadn vs_f, vo_f, vp.e0
    vmadh vs_i, vo_i, vp.e0
    vaddc vs_f, vs_f, vp.e3
    vadd vs_i, vs_i, vp.e2
    vmudn vs_f, vs_f, K16384
    vmadh vs_f, vs_i, K16384
    vge vs_f, vs_f, vzero
    vadd vs_f, vs_f, vs_f
    # W, 1 / W and S, T as the textured overlay writes them.
    vaddc vtmp, vo_f, vo_f
    vadd vk, vo_i, vo_i
    mfc2 t0, vk.e3
    mfc2 t1, vtmp.e3
    andi t1, 0xFFFF
    sll t0, 16
    or t0, t1
    srl t1, t0, 7
    subu t0, t1
    sw t0, 16(dst)
    mfc2 t0, vk.e7
    mfc2 t1, vtmp.e7
    andi t1, 0xFFFF
    sll t0, 16
    or t0, t1
    srl t1, t0, 7
    subu t0, t1
    sw t0, VTX_OUT_SIZE + 16(dst)
    ssv vinv_i.e3, 20, dst
    ssv vinv_f.e3, 22, dst
    ssv vinv_i.e7, VTX_OUT_SIZE + 20, dst
    ssv vinv_f.e7, VTX_OUT_SIZE + 22, dst
    llv vp.e0, 32, src
    llv vp.e4, VTX_IN_SIZE + 32, src
    llv vd.e0, 0xA0, dptr
    llv vd.e4, 0xA0, dptr
    llv vk.e0, 0xA4, dptr
    llv vk.e4, 0xA4, dptr
    vmudn vtmp, vk, vp
    vmadh vtmp, vd, vp
    llv vd.e0, 0xA8, dptr
    llv vd.e4, 0xA8, dptr
    vadd vtmp, vtmp, vd
    slv vtmp.e0, 0x40, wscr
    slv vtmp.e4, 0x44, wscr
    li t0, 0x003F                   # no clip flags, inside every plane
    sh t0, 6(dst)
    sh t0, VTX_OUT_SIZE + 6(dst)
    # A ripple ring: its vertex color, fogged (no light, Fresnel or glint;
    # the color load in the delay slot is the light's own too).
    lhu t0, 0x11C(wptr)
    bnez t0, water_fog
    luv wcol.e0, 24, src

    # The view direction v = (a, b, 1) zq, zq = 1 / sqrt(1 + a² + b²):
    # q = a² + b², (1 + q) / 4 and three Newton steps from 1 - q / 2 (the
    # screen's corners start far from the root).
    lqv wv9, 0x10, wscr             # a, b (s2.13)
    vmulf wv2, wv9, wv9             # a², b² (s4.11)
    vadd wv2, wv2, wv2.h1           # lane 0: q (s4.11)
    vadd wv2, wv2, wv2
    vadd wv2, wv2, wv2              # q / 4 (s0.15)
    vadd wv3, wv2, wvc.e7           # (1 + q) / 4
    vadd wv4, wv2, wv2              # q / 2
    vsub wv4, vzero, wv4
    jal water_newton
    vadd wv4, wv4, wvc.e1           # zq0 = 1 - q / 2
    jal water_newton
    nop
    jal water_newton
    nop                             # lane 0: zq
    vmulf wv5, wv9, wv4.h0
    vadd wv5, wv5, wv5
    vadd wv5, wv5, wv5              # a zq, b zq (s0.15)
    vmov wv5.e2, wv4.e0             # v = (a zq, b zq, zq)
    vmov wv5.e6, wv4.e4
    vmov wv5.e3, vzero.e0
    vmov wv5.e7, vzero.e0
    # c = n . to_eye = -(v . nv); sn = nv . sun; ts = to_eye . sun.
    # Three-lane sums add the products' own lanes 1 and 2 (adding a lane
    # of the sum itself counted lane 1 twice).
    lqv wv9, 0x00, wscr             # nv
    vmulf wv3, wv5, wv9
    vadd wv2, wv3, wv3.h1
    vadd wv2, wv2, wv3.h2
    vsub wv2, vzero, wv2            # lane 0: c
    lqv wv6, 0x150, wptr            # sun (view)
    vmulf wv7, wv9, wv6
    vadd wv3, wv7, wv7.h1
    vadd wv3, wv3, wv7.h2           # lane 0: sn
    vmulf wv7, wv5, wv6
    vadd wv4, wv7, wv7.h1
    vadd wv4, wv4, wv7.h2
    vsub wv4, vzero, wv4            # lane 0: ts
    # Fresnel: f = 0.85 (0.02 + 0.98 m^5) = 0.017 + 0.833 m^5, m = 1 - c+.
    vge wv7, wv2, vzero
    vsub wv7, vzero, wv7
    vadd wv7, wv7, wvc.e1           # m
    vmulf wv8, wv7, wv7
    vmulf wv8, wv8, wv8
    vmulf wv8, wv8, wv7             # m^5
    vmulf wv8, wv8, wvc.e4
    vadd wv8, wv8, wvc.e5           # lane 0: f
    # Glint: n . h with h = normalize(to_eye + sun) is (c + sn) / sqrt(2 (1
    # + ts)); squared, ((c + sn) / 2)² * 4 / (2 (1 + ts)), then raised to
    # the 8th (16th) power by squaring. None where c + sn <= 0.
    lhu t0, 0x114(wptr)
    vxor wv1, wv1, wv1
    beqz t0, water_no_glint
    vmulf wv2, wv2, wvc.e3
    vmulf wv3, wv3, wvc.e3
    vadd wv2, wv2, wv3              # e = (c + sn) / 2
    vge wv3, vzero, wv2             # VCC: e <= 0
    vmrg wv1, vzero, wv2            # e, or 0
    vmulf wv1, wv1, wv1             # e²
    vor wv6, vzero, K1              # 1
    vlt wv3, wv4, vzero
    vmrg wv6, vzero, wv6            # (1 + ts)'s integer part
    vaddc wv3, wv4, wv4             # (1 + ts)'s fraction: 2 ts
    vrcph wv6.e0, wv6.e0
    vrcpl wv3.e0, wv3.e0
    vrcph wv6.e0, vzero.e0          # 1 / (2 (1 + ts)), s15.16
    vrcph wv6.e4, wv6.e4
    vrcpl wv3.e4, wv3.e4
    vrcph wv6.e4, vzero.e0
    vaddc vtmp, vzero, vzero        # clear the carries
    vaddc wv1, wv1, wv1             # e² as an s15.16 fraction
    vmudl vtmp, wv3, wv1.h0
    vmadm vtmp, wv6, wv1.h0
    vmadn wv1, vzero, vzero         # e² / (2 (1 + ts)), fraction
    vadd wv1, wv1, wv1              # (n . h)² (s0.15)
    vmulf wv1, wv1, wv1
    vmulf wv1, wv1, wv1             # ^4 of the square: (n . h)^8
    lhu t0, 0x116(wptr)
    beqz t0, water_no_glint
    nop
    vmulf wv1, wv1, wv1             # (n . h)^16
water_no_glint:
    vmov wv8.e1, wv1.e0
    vmov wv8.e5, wv1.e4             # f, glint (lanes 0, 1), kept

    # Light, per channel, from the displaced normal (world): k = ambient
    # + diffuse * max(0, n . l).
    ldv vdiff.e0, 0x78, dptr
    ldv vdiff.e4, 0x78, dptr
    ldv vambc.e0, 0x70, dptr
    ldv vambc.e4, 0x70, dptr
    ldv vemis.e0, 0x80, dptr
    ldv vemis.e4, 0x80, dptr
    lqv vd, 0x30, wscr              # n . l (lane 1)
    vge vd, vd, vzero
    vmulf wk, vdiff, vd.h1
    vadd wk, wk, vambc
    vmulf vkt, vtint, wk
    vadd vkt, vkt, vta
    vmulf wcol, wcol, vkt
    vadd wcol, wcol, vemis          # emissive, saturating
    # The sky by the Fresnel term (alpha toward opaque too), the glint.
    lqv wv3, 0x160, wptr
    vsub wv3, wv3, wcol
    vmulf wv3, wv3, wv8.h0
    vadd wcol, wcol, wv3
    lqv wv3, 0x170, wptr
    vmulf wv3, wv3, wv8.h1
    vadd wcol, wcol, wv3
water_fog:
    # Fog: color += (fog - color) * f on red, green and blue.
    ldv vs_i.e0, 0x98, dptr
    ldv vs_i.e4, 0x98, dptr
    li t0, %lo(FOG_MASK2)
    lqv vinv_f, 0, t0
    vsub vs_i, vs_i, wcol
    vmulf vs_i, vs_i, vs_f.h3
    vand vs_i, vs_i, vinv_f
    vadd wcol, wcol, vs_i
    vge wcol, wcol, vzero
    suv wcol.e0, 8, dst             # A's RGBA, then B's over A's S, T
    lw t0, 12(dst)
    sw t0, VTX_OUT_SIZE + 8(dst)
    lw t0, 0x40(wscr)
    sw t0, 12(dst)                  # S, T
    lw t0, 0x44(wscr)
    sw t0, VTX_OUT_SIZE + 12(dst)
    addiu count, -2
    addiu src, 2 * VTX_IN_SIZE
    bgtz count, transform_vertex
    addiu dst, 2 * VTX_OUT_SIZE
    j vertices_done
    nop
#elif defined(TRINO_COLORED)
    # ------------------------------------------------------------------
    # Two vertices per pass, A in lanes 0..3 and B in lanes 4..7 (the
    # matrix, light and color terms are in both halves): the vector unit
    # waits on every dependent instruction, so one vertex's chain left it
    # idle most of the time. A segment with an odd count transforms one
    # vertex past its end, into a VTX_OUT slot no triangle reads.
    # ------------------------------------------------------------------
    ldv vpos.e0, 0, src
    ldv vpos.e4, VTX_IN_SIZE, src
    ldv pa_posf.e0, 8, src
    ldv pa_posf.e4, VTX_IN_SIZE + 8, src
    vmudl vtmp, vm0f, pa_posf.h0
    vmadm vtmp, vm0i, pa_posf.h0
    vmadn vtmp, vm0f, vpos.h0
    vmadh vtmp, vm0i, vpos.h0
    vmadl vtmp, vm1f, pa_posf.h1
    vmadm vtmp, vm1i, pa_posf.h1
    vmadn vtmp, vm1f, vpos.h1
    vmadh vtmp, vm1i, vpos.h1
    vmadl vtmp, vm2f, pa_posf.h2
    vmadm vtmp, vm2i, pa_posf.h2
    vmadn vtmp, vm2f, vpos.h2
    vmadh vtmp, vm2i, vpos.h2
    vmadn vo_f, vm3f, vpos.h3
    vmadh vo_i, vm3i, vpos.h3
    # 1 / w of both (lanes 3 and 7).
    vrcph vinv_i.e3, vo_i.e3
    vrcpl vinv_f.e3, vo_f.e3
    vrcph vinv_i.e3, vzero.e0
    vrcph vinv_i.e7, vo_i.e7
    vrcpl vinv_f.e7, vo_f.e7
    vrcph vinv_i.e7, vzero.e0
    # Screen = clip / w * scale + offset; Z from 1 / depth (see the single
    # vertex path below).
    vmudl vtmp, vo_f, vinv_f.h3
    vmadm vtmp, vo_i, vinv_f.h3
    vmadn vs_f, vo_f, vinv_i.h3
    vmadh vs_i, vo_i, vinv_i.h3
    vmudn vs_f, vs_f, vvs
    vmadh vs_i, vs_i, vvs
    vadd vs_i, vs_i, vvo
    vmudn vtmp, vinv_f, vvo.e3
    vmadh vtmp, vinv_i, vvo.e3
    vsub vtmp, vvs, vtmp.h3
    vmov vs_i.e2, vtmp.e2
    vmov vs_i.e6, vtmp.e6
    sdv vs_i.e0, 0, dst             # X, Y, Z (+ W in the flag bytes)
    sdv vs_i.e4, VTX_OUT_SIZE, dst
    # The fog factor from the view depth (twice w): f = w FS + FO (s15.16,
    # lanes 3 and 7), to s0.15 clamped to [0, 1] (s1.14 with saturation,
    # then doubled).
    ldv vd.e0, 0x88, dptr
    vmudl vtmp, vo_f, vd.e1
    vmadm vtmp, vo_i, vd.e1
    vmadn vs_f, vo_f, vd.e0
    vmadh vs_i, vo_i, vd.e0
    vaddc vs_f, vs_f, vd.e3
    vadd vs_i, vs_i, vd.e2
    vmudn vs_f, vs_f, K16384
    vmadh vs_f, vs_i, K16384
    vge vs_f, vs_f, vzero
    vadd vs_f, vs_f, vs_f
#ifdef TRINO_TEXTURED
    # W and 1 / W of each, as the single vertex path stores them.
    vaddc vtmp, vo_f, vo_f
    vadd vk, vo_i, vo_i
    mfc2 t0, vk.e3
    mfc2 t1, vtmp.e3
    andi t1, 0xFFFF
    sll t0, 16
    or t0, t1
    srl t1, t0, 7
    subu t0, t1
    sw t0, 16(dst)
    mfc2 t0, vk.e7
    mfc2 t1, vtmp.e7
    andi t1, 0xFFFF
    sll t0, 16
    or t0, t1
    srl t1, t0, 7
    subu t0, t1
    sw t0, VTX_OUT_SIZE + 16(dst)
    ssv vinv_i.e3, 20, dst
    ssv vinv_f.e3, 22, dst
    ssv vinv_i.e7, VTX_OUT_SIZE + 20, dst
    ssv vinv_f.e7, VTX_OUT_SIZE + 22, dst
#else
    sw zero, 16(dst)                # W, 1/W unused (untextured)
    sw zero, 20(dst)
    sw zero, VTX_OUT_SIZE + 16(dst)
    sw zero, VTX_OUT_SIZE + 20(dst)
#endif
    li t0, 0x003F                   # no clip flags, inside every plane
    sh t0, 6(dst)
    sh t0, VTX_OUT_SIZE + 6(dst)
    # Light, per channel: k = ambient + diffuse * max(0, n . l).
    ldv vpos.e0, 16, src
    ldv vpos.e4, VTX_IN_SIZE + 16, src
    vmulf pa_posf, vpos, vlight
    vadd vd, pa_posf, pa_posf.h1
    vadd vd, vd, pa_posf.h2
    vge vd, vd, vzero
    vmulf vk, pa_diff, vd.h0
    vadd vk, vk, pa_ambc
    vmulf vkt, vtint, vk
    vadd vkt, vkt, vta
    luv vcol.e0, 24, src            # A's color, then B's (A's pad bytes)
    vmulf vcol, vcol, vkt
    vadd vcol, vcol, pa_emis        # emissive, saturating
    # Fog: color += (fog - color) * f on red, green and blue.
    ldv vtmp.e0, 0x98, dptr
    ldv vtmp.e4, 0x98, dptr
    li t0, %lo(FOG_MASK2)
    lqv vinv_f, 0, t0
    vsub vtmp, vtmp, vcol
    vmulf vtmp, vtmp, vs_f.h3
    vand vtmp, vtmp, vinv_f
    vadd vcol, vcol, vtmp
    suv vcol.e0, 8, dst             # A's RGBA, then B's over A's S, T
    lw t0, 12(dst)
    sw t0, VTX_OUT_SIZE + 8(dst)
#ifdef TRINO_TEXTURED
    # S, T (s10.5) = u, v (s5.10) * texels per repeat / 32 + offset.
    llv vd.e0, 32, src
    llv vd.e4, VTX_IN_SIZE + 32, src
    llv vk.e0, 0xA4, dptr
    llv vk.e4, 0xA4, dptr
    llv vkt.e0, 0xA0, dptr
    llv vkt.e4, 0xA0, dptr
    vmudn vtmp, vk, vd
    vmadh vtmp, vkt, vd
    llv vk.e0, 0xA8, dptr
    llv vk.e4, 0xA8, dptr
    vadd vtmp, vtmp, vk
    slv vtmp.e0, 12, dst
    slv vtmp.e4, VTX_OUT_SIZE + 12, dst
#endif
    addiu count, -2
    addiu src, 2 * VTX_IN_SIZE
    bgtz count, transform_vertex
    addiu dst, 2 * VTX_OUT_SIZE
    j vertices_done
    nop
#else
    ldv vpos.e0, 0, src
    ldv vposf.e0, 8, src
    # Clip-space position = M * p, both s15.16 (32 x 32 bit, as in
    # libdragon's rspqdemo); w = 1 has no fractional part.
    vmudl vtmp, vm0f, vposf.e0
    vmadm vtmp, vm0i, vposf.e0
    vmadn vtmp, vm0f, vpos.e0
    vmadh vtmp, vm0i, vpos.e0
    vmadl vtmp, vm1f, vposf.e1
    vmadm vtmp, vm1i, vposf.e1
    vmadn vtmp, vm1f, vpos.e1
    vmadh vtmp, vm1i, vpos.e1
    vmadl vtmp, vm2f, vposf.e2
    vmadm vtmp, vm2i, vposf.e2
    vmadn vtmp, vm2f, vpos.e2
    vmadh vtmp, vm2i, vpos.e2
    vmadn vo_f, vm3f, vpos.e3
    vmadh vo_i, vm3i, vpos.e3

    # 1 / w.
    vrcph vinv_i.e3, vo_i.e3
    vrcpl vinv_f.e3, vo_f.e3
    vrcph vinv_i.e3, vzero.e0

    # Screen = clip / w * scale + offset.
    vmudl vtmp, vo_f, vinv_f.e3
    vmadm vtmp, vo_i, vinv_f.e3
    vmadn vs_f, vo_f, vinv_i.e3
    vmadh vs_i, vo_i, vinv_i.e3
    vmudn vs_f, vs_f, vvs
    vmadh vs_i, vs_i, vvs
    vadd vs_i, vs_i, vvo
    # Z = 0x7FFF - NEAR * 0x7FFF / depth: z / w through the 32-bit product
    # above loses ~9 bits on rotated meshes (measured, 58/32767), enough to
    # flip the depth test where objects touch; 1 / depth alone keeps it
    # within a unit.
    vmudn vtmp, vinv_f, vvo.e3
    vmadh vtmp, vinv_i, vvo.e3
    vsub vtmp, vvs, vtmp.e3
    vmov vs_i.e2, vtmp.e2
    sdv vs_i.e0, 0, dst             # X, Y, Z (+ W in the flag bytes)
    li t0, 0x003F                   # no clip flags, inside every plane
    sh t0, 6(dst)
    sw zero, 16(dst)                # W, 1/W unused (untextured)
    sw zero, 20(dst)

    # Light: k = ambient + (1 - ambient) * max(0, n . l).
    ldv vn.e0, 16, src
    vmulf vp, vn, vlight
    vadd vd, vp, vp.e1
    vadd vd, vd, vp.e2
    vge vd, vd, vzero
    vmulf vk, vd, vamb.e1
    vadd vk, vk, vamb.e0
    vmulf vkt, vtint, vk.e0
    vadd vkt, vkt, vta
    luv vcol.e0, 24, src
    vmulf vcol, vcol, vkt
    suv vcol.e0, 8, dst             # RGBA (+ 4 junk bytes over S/T)
#endif

    addiu count, -1
    addiu src, VTX_IN_SIZE
    bgtz count, transform_vertex
    addiu dst, VTX_OUT_SIZE
vertices_done:

    # Debug: segment 0's screen vertices back to RDRAM (the CPU checks
    # them against trino_core's projection).
    lw s0, %lo(DRAW) + 0x68
    beqz s0, 1f
    lhu t0, 6(seg_ptr)
    sll t1, t0, 4
    sll t0, t0, 3
    addu t0, t1                     # count * 24
    addiu t0, -1
    li s4, %lo(VTX_OUT)
    jal DMAOut
    nop
    sw zero, %lo(DRAW) + 0x68       # first segment only
1:

    # Triangles: 3 local u8 indices each, after the vertices.
    lhu t0, 6(seg_ptr)
#ifdef TRINO_TEXTURED
    sll t1, t0, 3
    sll t0, t0, 5
    addu t0, t1                     # count * VTX_IN_SIZE (40)
#else
    sll t0, t0, 5                   # count * VTX_IN_SIZE
#endif
    addiu idx, t0, %lo(VTX_IN)
    lhu count, 8(seg_ptr)
    beqz count, segment_done
    nop

draw_triangle:
    lbu a1, 0(idx)
    lbu a2, 1(idx)
    lbu a3, 2(idx)
    sll t0, a1, 4
    sll a1, a1, 3
    addu a1, t0
    sll t0, a2, 4
    sll a2, a2, 3
    addu a2, t0
    sll t0, a3, 4
    sll a3, a3, 3
    addu a3, t0
    addiu a1, %lo(VTX_OUT)
    addiu a2, %lo(VTX_OUT)
    addiu a3, %lo(VTX_OUT)
    lhu a0, %lo(DRAW) + 0x0A        # RDP triangle command (shade + z)
    lbu v0, %lo(DRAW) + 0x03        # cull mode
    jal RDPQ_Triangle_Send_Async
    li s3, %lo(TRI_STAGING)
RDPQ_Triangle_Clip:
RDPQ_Triangle_Cull:
    addiu count, -1
    bgtz count, draw_triangle
    addiu idx, 3

segment_done:
    addiu seg_left, -1
    bgtz seg_left, draw_segment
    nop

trino_test_done:
    jal_and_j RDPQ_Triangle_Send_End, RSPQ_Loop
    .endfunc

#ifdef TRINO_WATER
    # $v11 = water::sin_turns($v11) lane by lane (the turn's fraction,
    # read as s0.15 in [-1, 1)): y = 4u(1 - |u|), sin = y + P (y|y| - y).
    # $v12 is scratch; WCONST in wvc.
water_sin:
    vabs $v12, $v11, $v11
    vsub $v12, $v12, wvc.e1
    vmulf $v12, $v11, $v12
    vsub $v12, vzero, $v12
    vadd $v12, $v12, $v12
    vadd $v12, $v12, $v12
    vabs $v11, $v12, $v12
    vmulf $v11, $v12, $v11
    vsub $v11, $v11, $v12
    vmulf $v11, $v11, wvc.e2
    jr ra
    vadd $v11, $v12, $v11

    # One Newton step toward 1 / sqrt(4 wv3) in wv4 (s0.15): w += w / 2 -
    # 2w (w² wv3), the subtraction first (w + w / 2 alone saturates).
    # wv6, wv7 are scratch.
water_newton:
    vmulf wv6, wv4, wv4
    vmulf wv6, wv6, wv3
    vmulf wv6, wv4, wv6
    vadd wv6, wv6, wv6
    vmulf wv7, wv4, wvc.e3
    vsub wv4, wv4, wv6
    jr ra
    vadd wv4, wv4, wv7
#endif

#include <rsp_rdpq.inc>
