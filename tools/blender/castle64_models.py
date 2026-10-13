# castle64's hand-authored models, built in Blender from code (reproducible):
#
#   blender --background --factory-startup --python tools/blender/castle64_models.py -- [--only a,b] [--no-render]
#   (or: cargo xtask blender castle64 [--only a,b] [--no-render])
#
# Writes assets/shared/models/castle64/{door_castle,door_frame,boar,player_torso,
# player_arm,player_leg}.glb, gates each one with the engine's bake
# (`cargo xtask model inspect`) against a triangle budget, and renders QA sheets
# into target/blender_castle64/. Prints one `TRINO_BLENDER_MODELS {...}` JSON line.
#
# Why these exist (docs/adr/0006, n64-performance skill): on the N64 the CPU T&L
# pays ~9 us per vertex and ~44 us per source triangle EVEN WHEN THE TRIANGLE IS
# HIDDEN, because hiding happens later (back faces after the transform, buried
# faces only in the z-buffer). The old generated models were overlapping boxes,
# and every buried face still paid. The castle entrance was KayKit's
# 1068-triangle doorway (~64 ms per frame on the N64 for ~20 px of screen).
# Each model here is built from faces that can actually be seen:
#   - boxes in Trino coordinates (X right, Y up, +Z = the model's front); a box
#     face lying entirely inside another box is dropped, and so are faces no
#     camera reaches (soles on the ground, the back of a doorway set against a
#     wall). Boxes still interpenetrate: every target z-buffers.
#   - NOT a boolean union: it cuts every face another box passes through (the
#     boar's belly gets four holes for the legs) and the triangulation then costs
#     MORE triangles than the overlapping boxes (measured: boar 120 -> 184).
#   - hand-placed polygons where a shape needs it (the doorway's arch).
# Per-face colors are baked into COLOR_0 (linear, as glTF wants; the bake
# converts back to display bytes). No materials are exported: a material's base
# color would multiply the vertex colors a second time.
import bpy, bmesh, json, math, os, sys, traceback

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
OUT = os.path.join(REPO, "assets", "shared", "models", "castle64")


def T(p):
    """Trino (Y-up, +Z front) -> Blender (Z-up, front = -Y)."""
    return (p[0], -p[2], p[1])


def _lin(c):
    c = c / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def _sub(a, b):
    return (a[0] - b[0], a[1] - b[1], a[2] - b[2])


def _cross(a, b):
    return (a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0])


def _unit(a):
    n = math.sqrt(a[0] ** 2 + a[1] ** 2 + a[2] ** 2) or 1.0
    return (a[0] / n, a[1] / n, a[2] / n)


class Model:
    """Faces in Trino coordinates, CCW seen from outside, one color each."""

    def __init__(self, name):
        self.name = name
        self.faces = []   # (points, rgb, solid index or None)
        self.solids = []  # (min, max) of every box, for the buried-face test

    def poly(self, pts, rgb, out):
        """A planar polygon, flipped if needed so its front faces `out`."""
        pts = list(pts)
        n = self.normal(pts) if len(pts) == 3 else self._newell(pts)
        if n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0:
            pts.reverse()
        self.faces.append((pts, rgb, None))

    @staticmethod
    def _newell(pts):
        """Polygon normal robust to concave outlines (Newell's method)."""
        n = [0.0, 0.0, 0.0]
        for a, b in zip(pts, pts[1:] + pts[:1]):
            n[0] += (a[1] - b[1]) * (a[2] + b[2])
            n[1] += (a[2] - b[2]) * (a[0] + b[0])
            n[2] += (a[0] - b[0]) * (a[1] + b[1])
        return _unit(n)

    def box(self, center, size, rgb, skip=()):
        """Axis-aligned box; `skip` names faces never to build: '-x', '+y', ..."""
        lo = tuple(center[k] - size[k] / 2 for k in range(3))
        hi = tuple(center[k] + size[k] / 2 for k in range(3))
        sid = len(self.solids)
        self.solids.append((lo, hi))
        x0, y0, z0 = lo
        x1, y1, z1 = hi
        faces = {
            "-x": [(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)],
            "+x": [(x1, y0, z1), (x1, y0, z0), (x1, y1, z0), (x1, y1, z1)],
            "-y": [(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)],
            "+y": [(x0, y1, z1), (x1, y1, z1), (x1, y1, z0), (x0, y1, z0)],
            "-z": [(x1, y0, z0), (x0, y0, z0), (x0, y1, z0), (x1, y1, z0)],
            "+z": [(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)],
        }
        for key, pts in faces.items():
            if key not in skip:
                self.faces.append((pts, rgb, sid))

    @staticmethod
    def normal(pts):
        return _unit(_cross(_sub(pts[1], pts[0]), _sub(pts[2], pts[0])))

    def buried(self, pts, sid):
        """Entirely inside another box (probed just outside the face, corners
        pulled 1% toward the center) — no camera can ever see it."""
        n = self._newell(pts)
        c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
        probes = [tuple(p[k] + (c[k] - p[k]) * 0.01 + n[k] * 1e-3 for k in range(3)) for p in pts]
        for j, (lo, hi) in enumerate(self.solids):
            if j == sid:
                continue
            if all(all(lo[k] - 1e-6 <= q[k] <= hi[k] + 1e-6 for k in range(3)) for q in probes):
                return True
        return False

    def build(self, drop=None):
        me = bpy.data.meshes.new(self.name)
        bm = bmesh.new()
        col = bm.loops.layers.float_color.new("Col")
        kept = 0
        for pts, rgb, sid in self.faces:
            if sid is not None and self.buried(pts, sid):
                continue
            c = tuple(sum(p[k] for p in pts) / len(pts) for k in range(3))
            if drop and drop(c, self._newell(pts)):
                continue
            f = bm.faces.new([bm.verts.new(T(p)) for p in pts])
            lin = tuple(_lin(v) for v in rgb) + (1.0,)
            for loop in f.loops:
                loop[col] = lin
            kept += 1
        bmesh.ops.triangulate(bm, faces=bm.faces[:], quad_method="BEAUTY", ngon_method="BEAUTY")
        # Every polygon owns its vertices, so "smooth" normals ARE the face
        # normal — and the exporter then shares a polygon's vertices between
        # its triangles. Flat-shaded faces export 3 vertices per triangle
        # (measured: boar 70 tris -> 210 verts), at ~9 us each on the N64.
        for f in bm.faces:
            f.smooth = True
        bm.to_mesh(me)
        bm.free()
        me.color_attributes.active_color = me.color_attributes["Col"]
        try:
            me.color_attributes.render_color_index = me.color_attributes.active_color_index
        except AttributeError:
            pass
        ob = bpy.data.objects.new(self.name, me)
        bpy.context.scene.collection.objects.link(ob)
        return ob


def on_ground(c, n):
    """Faces looking down at y = 0 (soles, bases standing on the floor)."""
    return n[1] < -0.99 and abs(c[1]) < 1e-3


# ---------------------------------------------------------------- models

def door_castle():
    """The castle entrance: KayKit Dungeon Remastered's wall_doorway (CC0) as
    the reference — same 4 x 4 x 1 m extents, origin at the base center and
    the palette sampled from its texture — rebuilt by hand at ~1/14 of its
    triangles. It stands against the castle facade (yaw = pi, 9 cm in front of
    the wall): its back, its base and the cornice's underside are never seen."""
    wall, reveal, trim, plinth = (126, 136, 141), (104, 113, 118), (89, 96, 100), (75, 82, 86)
    wood, band, iron = (150, 85, 64), (110, 58, 42), (160, 166, 170)
    m = Model("door_castle")
    zf, zl = 0.25, 0.02          # wall front, door leaf front
    w2, spring, rise = 1.05, 2.1, 0.75
    fwd = (0, 0, 1)
    # Arch, left to right over the top (6 segments).
    arc = [(-w2 * math.cos(math.pi * i / 6), spring + rise * math.sin(math.pi * i / 6)) for i in range(7)]
    # Wall front around the opening, in convex pieces (Blender's n-gon fill
    # mangles the U-shaped outline: back-facing triangles across the
    # opening): two piers, then the spandrel above the arch as two fans from
    # the top corners over the arch points.
    m.poly([(-2, 0, zf), (-w2, 0, zf), (-w2, 3.3, zf), (-2, 3.3, zf)], wall, fwd)
    m.poly([(w2, 0, zf), (2, 0, zf), (2, 3.3, zf), (w2, 3.3, zf)], wall, fwd)
    for corner, pts in (((-w2, 3.3), arc[:4]), ((w2, 3.3), arc[3:])):
        for (xa, ya), (xb, yb) in zip(pts, pts[1:]):
            m.poly([(xa, ya, zf), (xb, yb, zf), (corner[0], corner[1], zf)], wall, fwd)
        m.poly([(0.0, spring + rise, zf), (0.0, 3.3, zf), (corner[0], corner[1], zf)], wall, fwd)
    # Wall ends above the plinths.
    m.poly([(-2, 0.5, -0.25), (-2, 0.5, zf), (-2, 3.3, zf), (-2, 3.3, -0.25)], wall, (-1, 0, 0))
    m.poly([(2, 0.5, -0.25), (2, 0.5, zf), (2, 3.3, zf), (2, 3.3, -0.25)], wall, (1, 0, 0))
    # Reveal: the opening's sides and arch soffit, from the leaf to the wall
    # face, each facing into the opening.
    jambs = [(-w2, 0.0)] + arc + [(w2, 0.0)]
    for (xa, ya), (xb, yb) in zip(jambs, jambs[1:]):
        inward = (-(xa + xb) / 2, 1.2 - (ya + yb) / 2, 0)
        m.poly([(xa, ya, zl), (xa, ya, zf), (xb, yb, zf), (xb, yb, zl)], reveal, inward)
    # Door leaf: plank field with two dark iron bands (coplanar strips of one
    # surface, so nothing to z-fight) under the arch top.
    rows = [(0.0, 0.59, wood), (0.59, 0.71, band), (0.71, 1.69, wood), (1.69, 1.81, band), (1.81, spring, wood)]
    for y0, y1, rgb in rows:
        m.poly([(-w2, y0, zl), (w2, y0, zl), (w2, y1, zl), (-w2, y1, zl)], rgb, fwd)
    m.poly([(x, y, zl) for x, y in arc], wood, fwd)
    # Ring pull: a low iron pyramid.
    rx, ry, r = 0.55, 1.2, 0.09
    apex = (rx, ry, zl + 0.08)
    sq = [(rx - r, ry - r, zl), (rx + r, ry - r, zl), (rx + r, ry + r, zl), (rx - r, ry + r, zl)]
    for p0, p1 in zip(sq, sq[1:] + sq[:1]):
        out = ((p0[0] + p1[0]) / 2 - rx, (p0[1] + p1[1]) / 2 - ry, 0.05)
        m.poly([p0, p1, apex], iron, out)
    # Cornice and plinths (boxes; buried faces fall out on their own).
    m.box((0, 3.65, 0), (4.0, 0.7, 1.0), trim, skip=("-y", "-z"))
    m.box((-1.55, 0.25, 0), (0.9, 0.5, 0.8), plinth, skip=("-y", "-z"))
    m.box((1.55, 0.25, 0), (0.9, 0.5, 0.8), plinth, skip=("-y", "-z"))
    return m.build()


def door_frame():
    """Neutral stage door (the game tints it per destination): posts, lintel,
    recessed slab and a knob. The free-standing doors are seen from behind, so
    only the base goes."""
    frame, slab, knob = (225, 220, 210), (245, 242, 235), (90, 90, 95)
    m = Model("door_frame")
    m.box((0, 0.9, 0), (1.1, 1.8, 0.12), slab)
    m.box((-0.62, 1.0, 0), (0.15, 2.0, 0.2), frame)
    m.box((0.62, 1.0, 0), (0.15, 2.0, 0.2), frame)
    m.box((0, 2.05, 0), (1.4, 0.15, 0.2), frame)
    m.box((0.35, 0.95, 0.08), (0.1, 0.1, 0.06), knob, skip=("-z",))
    return m.build(drop=on_ground)


def boar():
    """Patrol boar: body, head, snout, tusks, stub legs. The camera always
    looks down on it (it stands on the ground or on a platform top), so the
    17 cm under its belly is never seen: no belly/head/snout/tusk bottoms, no
    tusk backs, and the legs keep only their outer, inner and outward-facing
    ends — the faces looking at each other across the belly go.
    (The old model also had a mane box buried inside the body: 12 triangles
    nobody ever saw.)"""
    hide, dark, tusk = (105, 75, 55), (70, 50, 38), (235, 230, 215)
    m = Model("boar")
    m.box((0, 0.34, -0.05), (0.55, 0.34, 0.62), hide, skip=("-y",))
    m.box((0, 0.30, 0.34), (0.34, 0.26, 0.22), hide, skip=("-y",))
    m.box((0, 0.24, 0.48), (0.16, 0.12, 0.10), dark, skip=("-y",))
    m.box((-0.11, 0.20, 0.44), (0.04, 0.10, 0.04), tusk, skip=("-y", "-z"))
    m.box((0.11, 0.20, 0.44), (0.04, 0.10, 0.04), tusk, skip=("-y", "-z"))
    for x in (-0.17, 0.17):
        m.box((x, 0.09, 0.22), (0.10, 0.18, 0.10), dark, skip=("-y", "-z"))
        m.box((x, 0.09, -0.30), (0.10, 0.18, 0.10), dark, skip=("-y", "+z"))
    return m.build()


def player_torso():
    purple, skin, hair, nose = (140, 70, 195), (245, 200, 160), (55, 40, 70), (230, 180, 140)
    m = Model("player_torso")
    m.box((0, 0.52, 0), (0.50, 0.36, 0.34), purple)
    m.box((0, 0.82, 0), (0.38, 0.28, 0.34), skin)
    # Nose and hair cap undersides only show from below the chin.
    m.box((0, 0.80, 0.21), (0.10, 0.08, 0.08), nose, skip=("-y",))
    m.box((0, 0.99, 0), (0.40, 0.10, 0.36), hair, skip=("-y",))
    m.box((0, 0.90, -0.19), (0.40, 0.20, 0.06), hair)
    return m.build()


def player_arm():
    sleeve, skin = (170, 110, 225), (245, 200, 160)
    m = Model("player_arm")
    m.box((0, -0.16, 0), (0.13, 0.32, 0.13), sleeve)
    m.box((0, -0.36, 0), (0.11, 0.10, 0.11), skin)
    return m.build()


def player_leg():
    pants, shoe = (80, 50, 125), (45, 35, 55)
    m = Model("player_leg")
    m.box((0, -0.17, 0), (0.16, 0.34, 0.20), pants)
    m.box((0, -0.36, 0.03), (0.17, 0.08, 0.26), shoe)
    return m.build()


# name -> (builder, max triangles, QA framing (target, ortho))
MODELS = {
    "door_castle": (door_castle, 76, ((0, 0, 2), 4.6)),
    "door_frame": (door_frame, 44, ((0, 0, 1.05), 2.6)),
    "boar": (boar, 70, ((0, 0, 0.3), 1.2)),
    "player_torso": (player_torso, 50, ((0, 0, 0.7), 0.9)),
    "player_arm": (player_arm, 22, ((0, 0, -0.2), 0.6)),
    "player_leg": (player_leg, 22, ((0, 0, -0.2), 0.6)),
}


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    only = None
    if "--only" in argv:
        only = set(argv[argv.index("--only") + 1].split(","))
    render = "--no-render" not in argv
    qa = os.path.join(REPO, "target", "blender_castle64")
    os.makedirs(qa, exist_ok=True)
    global ns
    ns = {"__name__": "trino_tools", "SCRATCH": qa, "REPO": REPO}
    for f in ("qa_render.py", "color_tools.py", "rig_tools.py", "trino_export.py"):
        exec(open(os.path.join(REPO, "tools", "blender", f), encoding="utf-8").read(), ns)

    report = {"blender": bpy.app.version_string, "models": {}}
    ok = True
    for name, (build, budget, (target, ortho)) in MODELS.items():
        if only and name not in only:
            continue
        for o in list(bpy.data.objects):
            bpy.data.objects.remove(o, do_unlink=True)
        try:
            ob = build()
            path = os.path.join(OUT, name + ".glb")
            ns["export_glb"]([ob.name], path, animations=False)
            ins = ns["inspect"](path)
            tris = ins.get("triangles", 1 << 30)
            good = bool(ins.get("ok")) and tris <= budget
            report["models"][name] = {
                "ok": good, "triangles": tris, "budget": budget,
                "size": ins.get("size"), "warnings": ins.get("warnings"),
            }
            ok &= good
            if render:
                ns["vertex_color_mode"](True)
                tiles = [ns["shot"](os.path.join(qa, f"{name}_{az}_{el}.png"), az, el, target, ortho, res=240)
                         for az, el in ((0, 10), (45, 25), (90, 10), (180, 25), (0, 80), (30, -40))]
                ns["sheet"](os.path.join(qa, f"{name}.png"), tiles, 3)
                ns["orient_mode"](True, name)
                tiles = [ns["shot"](os.path.join(qa, f"{name}_o{az}_{el}.png"), az, el, target, ortho, res=200)
                         for az, el in ((0, 10), (90, 10), (180, 10), (270, 10), (0, 80), (0, -60))]
                ns["sheet"](os.path.join(qa, f"{name}_orient.png"), tiles, 3)
                ns["orient_mode"](False, name)
        except Exception:
            ok = False
            report["models"][name] = {"ok": False, "error": traceback.format_exc()}
    report["ok"] = ok
    print("TRINO_BLENDER_MODELS " + json.dumps(report))
    sys.stdout.flush()
    if not ok:
        sys.exit(1)


main()
