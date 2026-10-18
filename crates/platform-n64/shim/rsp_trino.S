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
#ifdef TRINO_WATER
    # The water's constants: unused, 1 - epsilon, sin_turns' P (0.225),
    # 1/2, the Fresnel term's 0.85 * 0.98 and 0.85 * 0.02, unused, 1/4.
    .align 4
WCONST:         .half 0x8000, 0x7FFF, 7373, 0x4000, 27296, 557, 0xFFFF, 0x2000
#endif

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
    #   +0xB0 the water block (RDRAM, WBLK below)
    #   +0xB4 the ripples' per-vertex deltas (RDRAM), each segment's from its
    #         first vertex (segment entry +12)
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
    # The water block (the CPU's `rsp::water_block`, one DMA per draw):
    #   +0x00 world x, y, z rows (lanes 5..7 of each matrix column), integer
    #         parts, column by column; +0x18 the fractional parts
    #   +0x30 UP: clip, fog and world per world unit of height (8 lanes),
    #         integer parts; +0x40 fractional parts
    #   +0x50 AX, BZ, C (int, frac pairs): the swell's phases (turns) in x, z
    #   +0x80 AMP, +0x88 GX, +0x90 GZ: height and half gradient per sine
    #   +0x98 8192 / fx, -8192 / fy: a, b (s2.13) from the screen math
    #   +0xA0 RC0, RC1, RC2: the view's rotation columns; +0xB8 sun (view)
    #   +0xC0 sky r, g, b, 255 (u8 << 7); +0xC8 glint color (u8 << 7)
    #   +0xD0 glint on, glint ^16 (halves)
    .align 4
WBLK:           .ds.b 0xE0
    .align 4
WSCR:           .ds.b 0x30         // per vertex: n, (a, b), S T, (f, glint)
    .align 4
RIP_IN:         .ds.b SEG_VERTS * 8
#endif

    .text

    .func TrinoCmd_TestTri
TrinoCmd_TestTri:
    andi a0, 0xFFFF
    li s3, %lo(TRI_STAGING)
    li v0, 2                        # no culling
    li a1, %lo(TEST_VTX) + 0
    li a2, %lo(TEST_VTX) + 24
    jal RDPQ_Triangle_Send_Async
    li a3, %lo(TEST_VTX) + 48
    j trino_test_done
    nop
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
    # The draw's water block (WBLK).
    lw s0, %lo(DRAW) + 0xB0
    li s4, %lo(WBLK)
    jal DMAIn
    li t0, 0xE0 - 1
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
#ifdef TRINO_COLORED
    // The per-channel light terms, reloaded per vertex from DRAW into
    // registers the screen math is done with.
    #define vdiff $v02
    #define vambc $v03
    #define vemis $v04
    #define dptr  t5
#endif

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
#ifdef TRINO_COLORED
    # The fog row in lane 4 of each column: the product below then gives
    # the fog factor (s15.16) in lane 4 of the clip vector, for free.
    addiu t1, t0, 0x88
    lsv vm0i.e4, 0x0, t1
    lsv vm1i.e4, 0x2, t1
    lsv vm2i.e4, 0x4, t1
    lsv vm3i.e4, 0x6, t1
    lsv vm0f.e4, 0x8, t1
    lsv vm1f.e4, 0xA, t1
    lsv vm2f.e4, 0xC, t1
    lsv vm3f.e4, 0xE, t1
#endif
    li t1, %lo(VIEW_SCALE)
    lqv vvs, 0, t1
    lqv vvo, 0x10, t1
    # Tint alpha alone in lane 3: kt = tint_rgb * k + (0, 0, 0, tint_a).
    vxor vta, vta, vta
    vmov vta.e3, vamb.e2

#ifdef TRINO_WATER
    #define wptr t6
    #define rptr t7
    #define wscr t8
    # World x, y, z in lanes 5..7 of each column.
    li t1, %lo(WBLK)
    lsv vm0i.e5, 0x00, t1
    lsv vm0i.e6, 0x02, t1
    lsv vm0i.e7, 0x04, t1
    lsv vm1i.e5, 0x06, t1
    lsv vm1i.e6, 0x08, t1
    lsv vm1i.e7, 0x0A, t1
    lsv vm2i.e5, 0x0C, t1
    lsv vm2i.e6, 0x0E, t1
    lsv vm2i.e7, 0x10, t1
    lsv vm3i.e5, 0x12, t1
    lsv vm3i.e6, 0x14, t1
    lsv vm3i.e7, 0x16, t1
    lsv vm0f.e5, 0x18, t1
    lsv vm0f.e6, 0x1A, t1
    lsv vm0f.e7, 0x1C, t1
    lsv vm1f.e5, 0x1E, t1
    lsv vm1f.e6, 0x20, t1
    lsv vm1f.e7, 0x22, t1
    lsv vm2f.e5, 0x24, t1
    lsv vm2f.e6, 0x26, t1
    lsv vm2f.e7, 0x28, t1
    lsv vm3f.e5, 0x2A, t1
    lsv vm3f.e6, 0x2C, t1
    lsv vm3f.e7, 0x2E, t1
    # The segment's ripple deltas, from its first vertex (entry +12) on.
    lw s0, %lo(DRAW) + 0xB4
    lw t0, 12(seg_ptr)
    sll t0, 3
    addu s0, t0
    li s4, %lo(RIP_IN)
    lhu t0, 6(seg_ptr)
    sll t0, 3
    jal DMAIn
    addiu t0, -1
    li rptr, %lo(RIP_IN)
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
    # A water vertex (ADR 0015, issue #69): the CPU path's WATER copy of
    # the T&L in fixed point. Displaced along world Y by the swell (two
    # sines, here) and the ripples (deltas the CPU wrote for the vertices
    # their bands reach), then lit from the displaced normal, the Fresnel
    # sky and the sun's glint added, fogged and textured.
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
    ldv vpos.e0, 0, src
    ldv vposf.e0, 8, src
    # Clip x, y, z, w, the fog factor and world x, y, z (lanes 0..7).
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

    # The swell's phases in turns (s15.16): T = AX * x + BZ * z + C, lanes
    # sin0, cos0 (+ 1/4 turn), sin1, cos1. Only the fraction matters.
    ldv wv2.e0, 0x50, wptr
    ldv wv3.e0, 0x58, wptr
    ldv wv4.e0, 0x60, wptr
    ldv wv5.e0, 0x68, wptr
    ldv wv6.e0, 0x70, wptr
    ldv wv7.e0, 0x78, wptr
    vmudl vtmp, wv3, vo_f.e5
    vmadm vtmp, wv2, vo_f.e5
    vmadn vtmp, wv3, vo_i.e5
    vmadh vtmp, wv2, vo_i.e5
    vmadl vtmp, wv5, vo_f.e7
    vmadm vtmp, wv4, vo_f.e7
    vmadn vtmp, wv5, vo_i.e7
    vmadh vtmp, wv4, vo_i.e7
    vmadn wv8, wv7, K1
    vmadh vtmp, wv6, K1
    # water::sin_turns: u = the fraction as s0.15 in [-1, 1) (it is the
    # signed reading of the turn's 16 fractional bits), y = 4u(1 - |u|),
    # sin = y + P (y|y| - y).
    li t0, %lo(WCONST)
    lqv wvc, 0, t0
    vabs wv9, wv8, wv8
    vsub wv9, wv9, wvc.e1
    vmulf wv9, wv8, wv9
    vsub wv9, vzero, wv9
    vadd wv9, wv9, wv9
    vadd wv9, wv9, wv9
    vabs wv8, wv9, wv9
    vmulf wv8, wv9, wv8
    vsub wv8, wv8, wv9
    vmulf wv8, wv8, wvc.e2
    vadd wv8, wv9, wv8              # sin0, cos0, sin1, cos1 (s0.15)
    # Height (m, s0.15) and half the gradient: sums of two lanes.
    ldv wv2.e0, 0x80, wptr
    vmulf wv2, wv8, wv2
    vadd wv2, wv2, wv2.e2           # lane 0: h
    ldv wv3.e0, 0x88, wptr
    vmulf wv3, wv8, wv3
    vadd wv3, wv3, wv3.e3           # lane 1: dh/dx / 2
    ldv wv4.e0, 0x90, wptr
    vmulf wv4, wv8, wv4
    vadd wv4, wv4, wv4.e3           # lane 1: dh/dz / 2
    vxor wv5, wv5, wv5
    vmov wv5.e0, wv2.e0
    vmov wv5.e1, wv3.e1
    vmov wv5.e2, wv4.e1
    ldv wv6.e0, 0, rptr             # the ripples' (h, gx / 2, gz / 2)
    vadd wv5, wv5, wv6              # G = (h, gx / 2, gz / 2, 0)

    # Displace: clip, fog and world y += h * UP. h as s15.16: fraction
    # 2h, integer -1 below zero.
    vsub wv6, vzero, K1             # -1
    vlt wv7, wv5, vzero
    vmrg wv6, wv6, vzero            # h's integer part
    vaddc wv7, wv5, wv5             # h's fraction
    lqv wv2, 0x30, wptr             # UP, integer parts
    lqv wv3, 0x40, wptr             # UP, fractional parts
    vmudn vtmp, vo_f, K1
    vmadh vtmp, vo_i, K1
    vmadl vtmp, wv3, wv7.e0
    vmadm vtmp, wv2, wv7.e0
    vmadn vo_f, wv3, wv6.e0
    vmadh vo_i, wv2, wv6.e0
    vaddc vtmp, vzero, vzero        # clear the carries (vadd adds them)

    # The normal (-gx, 1, -gz) / sqrt(1 + gx² + gz²). s = (g / 2)², and
    # w = 1 / sqrt(1 + 4s) from w0 = 1 - s by two Newton steps
    # w += w / 2 - 2w (w² (1 + 4s) / 4), the subtraction first: w + w / 2
    # alone saturates s0.15.
    vmulf wv2, wv5, wv5
    vadd wv2, wv2, wv2.e2           # lane 1: s
    vadd wv3, wv2, wvc.e7           # lane 1: (1 + 4s) / 4
    vsub wv4, vzero, wv2
    vadd wv4, wv4, wvc.e1           # lane 1: w0 = 1 - s
    vmulf wv6, wv4, wv4
    vmulf wv6, wv6, wv3
    vmulf wv6, wv4, wv6
    vadd wv6, wv6, wv6
    vmulf wv7, wv4, wvc.e3
    vsub wv4, wv4, wv6
    vadd wv4, wv4, wv7
    vmulf wv6, wv4, wv4
    vmulf wv6, wv6, wv3
    vmulf wv6, wv4, wv6
    vadd wv6, wv6, wv6
    vmulf wv7, wv4, wvc.e3
    vsub wv4, wv4, wv6
    vadd wv4, wv4, wv7              # lane 1: w
    vmulf wv6, wv5, wv4.e1
    vadd wv6, wv6, wv6
    vsub wv6, vzero, wv6            # lanes 1, 2: -gx w, -gz w
    vxor wv7, wv7, wv7
    vmov wv7.e0, wv6.e1
    vmov wv7.e1, wv4.e1
    vmov wv7.e2, wv6.e2
    sdv wv7.e0, 0, wscr             # n (world, s0.15)

    # 1 / w.
    vrcph vinv_i.e3, vo_i.e3
    vrcpl vinv_f.e3, vo_f.e3
    vrcph vinv_i.e3, vzero.e0
    vmudl vtmp, vo_f, vinv_f.e3
    vmadm vtmp, vo_i, vinv_f.e3
    vmadn vs_f, vo_f, vinv_i.e3
    vmadh vs_i, vo_i, vinv_i.e3
    # a, b = view x / depth, view y / depth (s2.13): the view direction.
    ldv wv5.e0, 0x98, wptr
    vmudn vtmp, vs_f, wv5
    vmadh wv5, vs_i, wv5
    sdv wv5.e0, 16, wscr
    vmudn vs_f, vs_f, vvs
    vmadh vs_i, vs_i, vvs
    vadd vs_i, vs_i, vvo
    vmudn vtmp, vinv_f, vvo.e3
    vmadh vtmp, vinv_i, vvo.e3
    vsub vtmp, vvs, vtmp.e3
    vmov vs_i.e2, vtmp.e2
    sdv vs_i.e0, 0, dst             # X, Y, Z (+ W in the flag bytes)
    # The fog factor (lane 4), s0.15 in [0, 1].
    vmudn vs_f, vo_f, vvs.e4
    vmadh vs_f, vo_i, vvs.e4
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
    ssv vinv_i.e3, 20, dst
    ssv vinv_f.e3, 22, dst
    llv vp.e0, 32, src
    llv vd.e0, 0xA0, dptr
    llv vk.e0, 0xA4, dptr
    vmudn vtmp, vk, vp
    vmadh vtmp, vd, vp
    llv vd.e0, 0xA8, dptr
    vadd vtmp, vtmp, vd
    slv vtmp.e0, 24, wscr
    li t0, 0x003F                   # no clip flags, inside every plane
    sh t0, 6(dst)

    # The view direction v = (a, b, 1) zq, zq = 1 / sqrt(1 + a² + b²):
    # q = a² + b², (1 + q) / 4 and three Newton steps from 1 - q / 2 (the
    # screen's corners start far from the root).
    li t0, %lo(WCONST)
    lqv wvc, 0, t0
    ldv wv9.e0, 16, wscr            # a, b (s2.13)
    vmulf wv2, wv9, wv9             # a², b² (s4.11)
    vadd wv2, wv2, wv2.e1           # lane 0: q (s4.11)
    vadd wv2, wv2, wv2
    vadd wv2, wv2, wv2              # q / 4 (s0.15)
    vadd wv3, wv2, wvc.e7           # (1 + q) / 4
    vadd wv4, wv2, wv2              # q / 2
    vsub wv4, vzero, wv4
    vadd wv4, wv4, wvc.e1           # zq0 = 1 - q / 2
    vmulf wv6, wv4, wv4
    vmulf wv6, wv6, wv3
    vmulf wv6, wv4, wv6
    vadd wv6, wv6, wv6
    vmulf wv7, wv4, wvc.e3
    vsub wv4, wv4, wv6
    vadd wv4, wv4, wv7
    vmulf wv6, wv4, wv4
    vmulf wv6, wv6, wv3
    vmulf wv6, wv4, wv6
    vadd wv6, wv6, wv6
    vmulf wv7, wv4, wvc.e3
    vsub wv4, wv4, wv6
    vadd wv4, wv4, wv7
    vmulf wv6, wv4, wv4
    vmulf wv6, wv6, wv3
    vmulf wv6, wv4, wv6
    vadd wv6, wv6, wv6
    vmulf wv7, wv4, wvc.e3
    vsub wv4, wv4, wv6
    vadd wv4, wv4, wv7              # lane 0: zq
    vmulf wv5, wv9, wv4.e0
    vadd wv5, wv5, wv5
    vadd wv5, wv5, wv5              # a zq, b zq (s0.15)
    vmov wv5.e2, wv4.e0             # v = (a zq, b zq, zq)
    vmov wv5.e3, vzero.e0
    # The normal in view space: RC0 nx + RC1 ny + RC2 nz.
    ldv wv1.e0, 0, wscr             # n
    ldv wv6.e0, 0xA0, wptr
    ldv wv7.e0, 0xA8, wptr
    ldv wv8.e0, 0xB0, wptr
    vmulf wv9, wv6, wv1.e0
    vmacf wv9, wv7, wv1.e1
    vmacf wv9, wv8, wv1.e2          # nv
    # c = n . to_eye = -(v . nv); sn = nv . sun; ts = to_eye . sun.
    # Three-lane sums add the products' own lanes 1 and 2 (adding a lane
    # of the sum itself counted lane 1 twice).
    vmulf wv3, wv5, wv9
    vadd wv2, wv3, wv3.e1
    vadd wv2, wv2, wv3.e2
    vsub wv2, vzero, wv2            # lane 0: c
    ldv wv6.e0, 0xB8, wptr          # sun (view)
    vmulf wv7, wv9, wv6
    vadd wv3, wv7, wv7.e1
    vadd wv3, wv3, wv7.e2           # lane 0: sn
    vmulf wv7, wv5, wv6
    vadd wv4, wv7, wv7.e1
    vadd wv4, wv4, wv7.e2
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
    lhu t0, 0xD0(wptr)
    vxor wv1, wv1, wv1
    beqz t0, water_no_glint
    vmulf wv2, wv2, wvc.e3
    vmulf wv3, wv3, wvc.e3
    vadd wv2, wv2, wv3              # e = (c + sn) / 2
    vge wv3, vzero, wv2             # VCC: e <= 0
    vmrg wv1, vzero, wv2            # e, or 0
    vmulf wv1, wv1, wv1             # e²
    vxor wv6, wv6, wv6
    vadd wv6, wv6, K1               # 1
    vlt wv3, wv4, vzero
    vmrg wv6, vzero, wv6            # (1 + ts)'s integer part
    vaddc wv3, wv4, wv4             # (1 + ts)'s fraction: 2 ts
    vrcph wv6.e0, wv6.e0
    vrcpl wv3.e0, wv3.e0
    vrcph wv6.e0, vzero.e0          # 1 / (2 (1 + ts)), s15.16
    vaddc vtmp, vzero, vzero        # clear the carries
    vaddc wv1, wv1, wv1             # e² as an s15.16 fraction
    vmudl vtmp, wv3, wv1.e0
    vmadm vtmp, wv6, wv1.e0
    vmadn wv1, vzero, vzero         # e² / (2 (1 + ts)), fraction
    vadd wv1, wv1, wv1              # (n . h)² (s0.15)
    vmulf wv1, wv1, wv1
    vmulf wv1, wv1, wv1             # ^4 of the square: (n . h)^8
    lhu t0, 0xD2(wptr)
    beqz t0, water_no_glint
    nop
    vmulf wv1, wv1, wv1             # (n . h)^16
water_no_glint:
    vmov wv8.e1, wv1.e0
    sdv wv8.e0, 32, wscr            # f, glint

    # Light, per channel, from the displaced normal (world): k = ambient
    # + diffuse * max(0, n . l).
    ldv vdiff.e0, 0x78, dptr
    ldv vambc.e0, 0x70, dptr
    ldv vemis.e0, 0x80, dptr
    ldv vn.e0, 0, wscr
    vmulf vp, vn, vlight
    vadd vd, vp, vp.e1
    vadd vd, vd, vp.e2
    vge vd, vd, vzero
    vmulf vk, vdiff, vd.e0
    vadd vk, vk, vambc
    vmulf vkt, vtint, vk
    vadd vkt, vkt, vta
    luv vcol.e0, 24, src
    vmulf vcol, vcol, vkt
    vadd vcol, vcol, vemis          # emissive, saturating
    # The sky by the Fresnel term (alpha toward opaque too), the glint.
    ldv wv2.e0, 32, wscr
    ldv wv3.e0, 0xC0, wptr
    vsub wv3, wv3, vcol
    vmulf wv3, wv3, wv2.e0
    vadd vcol, vcol, wv3
    ldv wv3.e0, 0xC8, wptr
    vmulf wv3, wv3, wv2.e1
    vadd vcol, vcol, wv3
    # Fog: color += (fog - color) * f on red, green and blue.
    ldv vs_i.e0, 0x98, dptr
    li t0, %lo(FOG_MASK)
    lqv vinv_f, 0, t0
    vsub vs_i, vs_i, vcol
    vmulf vs_i, vs_i, vs_f.e4
    vand vs_i, vs_i, vinv_f
    vadd vcol, vcol, vs_i
    vge vcol, vcol, vzero
    suv vcol.e0, 8, dst             # RGBA (+ 4 junk bytes over S/T)
    llv vtmp.e0, 24, wscr
    slv vtmp.e0, 12, dst            # S, T over the junk
    addiu rptr, 8
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
#ifdef TRINO_COLORED
    # The fog factor, lane 4 of the clip vector, to s0.15 clamped to
    # [0, 1]: (int << 16 | frac) * 0x4000 >> 16 saturates to s1.14, then
    # doubles with saturation (1.0 and beyond are 0x7FFF).
    vmudn vs_f, vo_f, vvs.e4
    vmadh vs_f, vo_i, vvs.e4
    vge vs_f, vs_f, vzero
    vadd vs_f, vs_f, vs_f
#ifdef TRINO_TEXTURED
    # W = view depth (twice the clip w: 32-bit add) and 1 / W (the
    # reciprocal above is already 1 / depth), s15.16, for the RDP's
    # perspective correction. RDPQ_Triangle_Send_Async normalizes 1 / W by
    # the triangle's smallest W and keeps only the fraction of the product:
    # vrcp's 1 / W can round up, and W * (1 / W) just over 1.0 came out
    # near 0, every texel the (0, 0) one (vrcp is off by up to ~0.2%,
    # measured at depth 1.25). W is stored 2^-7 smaller (the
    # same factor on every vertex: perspective only reads ratios).
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
    ssv vinv_i.e3, 20, dst
    ssv vinv_f.e3, 22, dst
    # S, T (s10.5) = u, v (s5.10) * texels per repeat / 32 + offset, in
    # vtmp until the color is stored over them.
    llv vp.e0, 32, src
    llv vd.e0, 0xA0, dptr
    llv vk.e0, 0xA4, dptr
    vmudn vtmp, vk, vp
    vmadh vtmp, vd, vp
    llv vd.e0, 0xA8, dptr
    vadd vtmp, vtmp, vd
#endif
    # The screen math is done: its registers take the per-channel terms.
    ldv vdiff.e0, 0x78, dptr
    li t0, 0x003F                   # no clip flags, inside every plane
    sh t0, 6(dst)
    ldv vambc.e0, 0x70, dptr
#ifndef TRINO_TEXTURED
    sw zero, 16(dst)                # W, 1/W unused (untextured)
    sw zero, 20(dst)
#endif
    ldv vemis.e0, 0x80, dptr

    # Light, per channel: k = ambient + diffuse * max(0, n . l).
    ldv vn.e0, 16, src
    vmulf vp, vn, vlight
    vadd vd, vp, vp.e1
    vadd vd, vd, vp.e2
    vge vd, vd, vzero
    vmulf vk, vdiff, vd.e0
    vadd vk, vk, vambc
    vmulf vkt, vtint, vk
    vadd vkt, vkt, vta
    luv vcol.e0, 24, src
    vmulf vcol, vcol, vkt
    vadd vcol, vcol, vemis          # emissive, saturating
    # Fog: color += (fog - color) * f on red, green and blue, as the CPU
    # path's lerp toward the fog color (alpha kept).
    ldv vs_i.e0, 0x98, dptr
    li t0, %lo(FOG_MASK)
    lqv vinv_f, 0, t0
    vsub vs_i, vs_i, vcol
    vmulf vs_i, vs_i, vs_f.e4
    vand vs_i, vs_i, vinv_f
    vadd vcol, vcol, vs_i
    suv vcol.e0, 8, dst             # RGBA (+ 4 junk bytes over S/T)
#ifdef TRINO_TEXTURED
    slv vtmp.e0, 12, dst            # S, T over the junk
#endif
#else
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
#endif

    addiu count, -1
    addiu src, VTX_IN_SIZE
    bgtz count, transform_vertex
    addiu dst, VTX_OUT_SIZE

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

#include <rsp_rdpq.inc>
