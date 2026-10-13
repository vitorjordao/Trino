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
# Mesh data (built once by the CPU at register, see the N64 renderer) is a
# table of segments of at most SEG_VERTS vertices; each segment is one
# DMA: its 32-byte vertices, then 3 local u8 indices per triangle.
#
#   vertex  +0  x, y, z, 1   s15.16 integer parts
#           +8  x, y, z, 0   s15.16 fractional parts
#           +16 nx, ny, nz, 0  s16, s0.15
#           +24 r, g, b, a, 0, 0, 0, 0  u8
##############################################################################

#include <rsp_queue.inc>
#include <rdpq_macros.h>

    .set noreorder
    .set at

#define SEG_VERTS       32
#define VTX_IN_SIZE     32
#define VTX_OUT_SIZE    24
#define MAX_SEGS        16
#define DRAW_CMD_SIZE   0x6C
#define TRI_SLOT        0xB0    // RDPQ_CMD_STAGING's size: the routine writes up to it

    .data

    RSPQ_BeginOverlayHeader
        RSPQ_DefineCommand TrinoCmd_TestTri, 4                  # 0x0
        RSPQ_DefineCommand TrinoCmd_Draw,    DRAW_CMD_SIZE      # 0x1
    RSPQ_EndOverlayHeader

    .align 4
BANNER: .ascii "Trino RSP T&L   "

    RSPQ_EmptySavedState

    # Viewport: clip space x/y in [-w, w] -> s13.2 screen coordinates of a
    # 320x240 frame (160*4, 120*4); z/w in [0, 1) -> 0..0x7FFF.
    .align 4
VIEW_SCALE:     .half 160*4, 120*4, 0x7FFF, 0, 0, 0, 0, 0
VIEW_OFFSET:    .half 160*4, 120*4, 0, 0, 0, 0, 0, 0

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
    #   +0x04 segment table (RDRAM)   +0x08 segment count | triangle command
    #   +0x0C unused                  +0x10 matrix columns, integer parts
    #   +0x30 matrix columns, fractional parts
    #   +0x50 light (s0.15, model space, toward the light) x, y, z, 0
    #   +0x58 tint r, g, b, 0 (s0.15)  +0x60 ambient, 1 - ambient, tint a, 0
    #   +0x68 debug: RDRAM address for segment 0's screen vertices (0 = off)
    .align 4
DRAW:           .ds.b 0x70
    .align 4
SEG_TABLE:      .ds.b MAX_SEGS * 16
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

    # Segment table into SEG_TABLE.
    lw s0, %lo(DRAW) + 0x04
    lhu seg_left, %lo(DRAW) + 0x08
    li s4, %lo(SEG_TABLE)
    sll t0, seg_left, 4
    jal DMAIn
    addiu t0, -1
    li seg_ptr, %lo(SEG_TABLE)

draw_segment:
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

    lhu count, 6(seg_ptr)
    li src, %lo(VTX_IN)
    li dst, %lo(VTX_OUT)

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
    sll t0, t0, 5                   # count * VTX_IN_SIZE
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
    addiu seg_ptr, 16

trino_test_done:
    jal_and_j RDPQ_Triangle_Send_End, RSPQ_Loop
    .endfunc

#include <rsp_rdpq.inc>
