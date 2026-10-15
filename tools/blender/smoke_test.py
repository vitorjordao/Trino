# End-to-end smoke test of the Blender toolkit against the engine's bake.
#
#   blender --background --factory-startup --python tools/blender/smoke_test.py -- <out_dir> [--no-render]
#   (or: cargo xtask blender smoke — finds Blender, skips cleanly without it)
#
# Builds, from nothing, exactly what the blender-asset skill describes:
#   1. a static prop ("Crate"): mesh, per-face colors with Painter, export,
#      `model inspect` gate (12 triangles, 1 m box, origin at the base);
#   2. a rigid-part character ("Bot"): body -> head/arms/legs with pivots,
#      `walk` (loop) and `wave` (once) clips via make_clip + stash_clips,
#      export, gate (6 joints, both clips, durations, loop flags);
#   3. the facing convention: an arrow whose nose points to Blender -Y is
#      checked to land on Trino +Z;
#   4. QA renders (turntable, orient_mode, clip_sheet) unless --no-render.
#
# Prints one JSON line `TRINO_BLENDER_SMOKE {...}` and exits non-zero on any
# failed gate.
import bpy, json, math, os, sys, traceback

REPO_ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    out_dir = os.path.abspath(argv[0]) if argv else os.path.join(REPO_ROOT, "target", "blender_smoke")
    render = "--no-render" not in argv
    os.makedirs(out_dir, exist_ok=True)

    ns = {"__name__": "trino_tools", "SCRATCH": out_dir, "REPO": REPO_ROOT}
    for f in ("qa_render.py", "color_tools.py", "rig_tools.py", "trino_export.py"):
        exec(open(os.path.join(REPO_ROOT, "tools", "blender", f), encoding="utf-8").read(), ns)

    # Empty scene.
    for o in list(bpy.data.objects):
        bpy.data.objects.remove(o, do_unlink=True)

    report = {"blender": bpy.app.version_string, "gates": {}}
    failures = []

    def gate(name, ok, detail):
        report["gates"][name] = {"ok": bool(ok), "detail": detail}
        if not ok:
            failures.append(name)

    def box(name, size, center, parent=None):
        bpy.ops.mesh.primitive_cube_add(size=1.0, location=center)
        o = bpy.context.active_object
        o.name = name
        o.data.name = name
        o.scale = size
        bpy.ops.object.transform_apply(location=False, rotation=False, scale=True)
        if parent is not None:
            o.parent = bpy.data.objects[parent]
            o.matrix_parent_inverse = o.parent.matrix_world.inverted()
        return o

    # ---- 1. static prop -------------------------------------------------
    crate = box("Crate", (1.0, 1.0, 1.0), (0, 0, 0.5))
    ns["set_pivot"]("Crate", (0, 0, 0))  # origin at the base
    P = ns["Painter"]("Crate")
    P.fill((176, 124, 78))
    P.paint(lambda c, n: n.z > 0.9, (214, 168, 112), "lid")
    P.noise(0.04)
    P.ambient_occlusion_floor(0.2)
    P.commit()
    gate("crate_report", ns["report"]("Crate")["triangles"] == 12, ns["report"]("Crate"))
    exp = ns["export_glb"](["Crate"], os.path.join(out_dir, "crate.glb"), animations=False)
    rep = ns["inspect"](exp["path"])
    size = rep.get("size") or []
    gate(
        "crate_inspect",
        rep.get("ok") and rep.get("triangles") == 12 and all(abs(s - 1.0) < 1e-3 for s in size),
        {k: rep.get(k) for k in ("ok", "triangles", "size", "warnings", "errors", "error")},
    )
    report["crate_skipped_options"] = exp["skipped_options"]
    if render:
        ns["vertex_color_mode"](True)
        ns["turntable"]("crate_turntable.png", (0, 0, 0.5), 2.0, res=120)
        ns["orient_mode"](True, "Crate")
        ns["shot"](os.path.join(out_dir, "crate_orient.png"), 30, 20, (0, 0, 0.5), 2.0, res=160)
        ns["orient_mode"](False, "Crate")
    bpy.data.objects.remove(crate, do_unlink=True)

    # ---- 2. rigid-part character ----------------------------------------
    box("Bot", (0.6, 0.4, 0.7), (0, 0, 1.05))
    ns["set_pivot"]("Bot", (0, 0, 0))
    box("head", (0.4, 0.4, 0.4), (0, 0, 1.65), "Bot")
    ns["set_pivot"]("head", (0, 0, 1.4))
    box("arm_l", (0.15, 0.15, 0.6), (0.4, 0, 1.1), "Bot")
    ns["set_pivot"]("arm_l", (0.4, 0, 1.35))
    box("arm_r", (0.15, 0.15, 0.6), (-0.4, 0, 1.1), "Bot")
    ns["set_pivot"]("arm_r", (-0.4, 0, 1.35))
    box("leg_l", (0.2, 0.2, 0.7), (0.15, 0, 0.35), "Bot")
    ns["set_pivot"]("leg_l", (0.15, 0, 0.7))
    box("leg_r", (0.2, 0.2, 0.7), (-0.15, 0, 0.35), "Bot")
    ns["set_pivot"]("leg_r", (-0.15, 0, 0.7))
    parts = ["Bot", "head", "arm_l", "arm_r", "leg_l", "leg_r"]
    for name, rgb in zip(parts, [(90, 60, 200), (230, 200, 160), (90, 60, 200),
                                 (90, 60, 200), (60, 50, 90), (60, 50, 90)]):
        p = ns["Painter"](name)
        p.fill(rgb)
        p.commit()
    pr = ns["part_report"]("Bot")
    gate("bot_part_report", pr["ok"] and pr["joints_if_animated"] == 6, pr["joints_if_animated"])

    ns["make_clip"]("walk", parts, 24, [
        (1, {"arm_l": [("X", 30)], "arm_r": [("X", -30)], "leg_l": [("X", -25)], "leg_r": [("X", 25)]}),
        (13, {"arm_l": [("X", -30)], "arm_r": [("X", 30)], "leg_l": [("X", 25)], "leg_r": [("X", -25)]}),
    ])
    ns["make_clip"]("wave", parts, 24, [
        (1, {"arm_r": [("Y", 0)]}),
        (12, {"arm_r": [("Y", -150)]}),
        (24, {"arm_r": [("Y", -120)]}),
    ], loop=False)
    stashed = ns["stash_clips"](["walk", "wave"], parts)
    gate("bot_stash", len(stashed) >= 4, len(stashed))
    exp = ns["export_glb"](["Bot"], os.path.join(out_dir, "bot.glb"))
    rep = ns["inspect"](exp["path"], fps=15, once=["wave"])
    clips = {c.get("name"): c for c in rep.get("clips", [])}
    gate(
        "bot_inspect",
        rep.get("ok")
        and rep.get("joints") == 6
        and "walk" in clips
        and "wave" in clips
        and clips["walk"].get("loop") is True
        and clips["wave"].get("loop") is False,
        {k: rep.get(k) for k in ("ok", "triangles", "joints", "clips", "warnings", "errors", "error")},
    )
    report["bot_skipped_options"] = exp["skipped_options"]
    if render:
        ns["vertex_color_mode"](True)
        ns["turntable"]("bot_turntable.png", (0, 0, 1), 2.6, res=120)
        walk = bpy.data.actions.get("walk__Bot")
        ns["clip_sheet"](bpy.data.objects["Bot"], walk, [1, 7, 13, 19],
                         [(90, 10)], "bot_walk.png", target=(0, 0, 1), ortho=2.5, res=120)
    for name in reversed(parts):
        bpy.data.objects.remove(bpy.data.objects[name], do_unlink=True)

    # ---- 3. facing convention -------------------------------------------
    # Nose 1.0 toward Blender -Y, tail 0.2 toward +Y.
    arrow = box("Arrow", (0.2, 1.2, 0.2), (0, -0.4, 0.1))
    ns["set_pivot"]("Arrow", (0, 0, 0))
    exp = ns["export_glb"](["Arrow"], os.path.join(out_dir, "arrow.glb"), animations=False)
    rep = ns["inspect"](exp["path"])
    bmin = rep.get("min") or [0, 0, 0]
    bsize = rep.get("size") or [0, 0, 0]
    bmax = [bmin[i] + bsize[i] for i in range(3)]
    gate(
        "facing_minus_y_is_plus_z",
        rep.get("ok") and abs(bmax[2] - 1.0) < 1e-3 and abs(bmin[2] + 0.2) < 1e-3,
        {"min": bmin, "max": bmax},
    )
    bpy.data.objects.remove(arrow, do_unlink=True)

    # ---- 4. color slots (`_SLOT`, recolored per draw by a palette) ------
    tee = box("Tee", (0.6, 0.4, 0.6), (0, 0, 0.3))
    P = ns["Painter"]("Tee")
    P.fill((255, 255, 255))  # slot regions are painted white
    P.paint(lambda c, n: n.z < -0.9, (60, 50, 90), "hem")
    P.commit()
    painted = ns["paint_slots"]("Tee", [(lambda c, n: n.z > 0.9, 1), (lambda c, n: abs(n.z) < 0.1, 2)])
    exp = ns["export_glb"](["Tee"], os.path.join(out_dir, "tee.glb"), animations=False)
    rep = ns["inspect"](exp["path"])
    # One top face in slot 1, four sides in slot 2, the bottom keeps its color.
    gate(
        "color_slots_export",
        rep.get("ok") and rep.get("slot_vertices") == [4, 4, 16, 0, 0],
        {"painted": painted, "slot_vertices": rep.get("slot_vertices"), "error": rep.get("error")},
    )
    bpy.data.objects.remove(tee, do_unlink=True)

    report["ok"] = not failures
    report["failed"] = failures
    report["out_dir"] = out_dir
    print("TRINO_BLENDER_SMOKE " + json.dumps(report))
    return 0 if not failures else 1


try:
    code = main()
except Exception:
    traceback.print_exc()
    print("TRINO_BLENDER_SMOKE " + json.dumps({"ok": False, "error": traceback.format_exc()[-2000:]}))
    code = 2
sys.exit(code)
