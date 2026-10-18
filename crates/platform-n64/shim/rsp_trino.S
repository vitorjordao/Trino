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
#if defined(TRINO_TEXTURED)
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
#if defined(TRINO_TEXTURED)
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

    lhu count, 6(seg_ptr)
    li src, %lo(VTX_IN)
    li dst, %lo(VTX_OUT)
#ifdef TRINO_COLORED
    li dptr, %lo(DRAW)
#endif

transform_vertex:
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
