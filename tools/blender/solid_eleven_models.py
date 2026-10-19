# Solid Eleven's models, built in Blender from code (reproducible):
#
#   blender --background --factory-startup --python tools/blender/solid_eleven_models.py -- [--only a,b] [--no-render]
#   (or: cargo xtask blender solid-eleven [--only a,b] [--no-render])
#
# Writes examples/solid-eleven/assets/shared/models/*.glb (the PC-only ones,
# `*_hd` variants and the split limbs, are `platforms = ["pc"]` in the
# manifest), gates each one with the engine's bake (`cargo xtask model inspect`)
# against a triangle budget, renders QA sheets into target/blender_solid_eleven/,
# and prints one `TRINO_BLENDER_MODELS {...}` JSON line.
#
# Design (docs/SOLID_ELEVEN.md, the n64-performance skill):
# - Everything the N64 draws stays on its RSP: opaque, untextured, vertex
#   colors, no palette, single-sided (two-sided panels are built as two faces).
#   Kits are a per-draw tint, so recolorable regions are painted light grey
#   with their shading in; fixed details (boots, hair, eyes) keep dark colors.
# - Players are rigid parts posed by the game (one code path on every target,
#   tools/blender/solid_eleven_anim.py BODY): consoles draw 9 near the camera
#   (head, torso, shorts, arms, shorts legs, legs), the PC 15 (elbows,
#   knees, sleeves); the blob shadow is a draw of its own on the turf.
# - The pitch is 24 tiles so off-screen ground culls and on-screen ground fits
#   the RSP's clip band; lines are separate strips 2 cm over the turf (the game
#   draws the world at 1/64 scale, so the N64's 16-bit z resolves them).
# - Stands repeat one section model per side (two crowd variants), so RAM holds
#   a handful of meshes; the PC override packs a denser, people-shaped crowd.
#
# Coordinates: Trino (x = pitch length / a model's left, y = up, z = across /
# a model's front); T() converts to Blender's Z-up, front = -Y.
import bpy, bmesh, json, math, os, random, sys, traceback

REPO = os.path.abspath(os.path.join(os.path.dirname(__file__), "..", ".."))
GAME = os.path.join(REPO, "examples", "solid-eleven")
SHARED = os.path.join(GAME, "assets", "shared", "models")
PC = SHARED  # PC-only models are `platforms = ["pc"]` in the manifest
sys.path.insert(0, os.path.dirname(__file__))
import solid_eleven_anim as anim  # noqa: E402


def T(p):
    return (p[0], -p[2], p[1])


def _lin(c):
    c = c / 255.0
    return c / 12.92 if c <= 0.04045 else ((c + 0.055) / 1.055) ** 2.4


def shade(rgb, k):
    return tuple(max(0, min(255, int(round(v * k)))) for v in rgb)


def _newell(pts):
    n = [0.0, 0.0, 0.0]
    for a, b in zip(pts, pts[1:] + pts[:1]):
        n[0] += (a[1] - b[1]) * (a[2] + b[2])
        n[1] += (a[2] - b[2]) * (a[0] + b[0])
        n[2] += (a[0] - b[0]) * (a[1] + b[1])
    ln = math.sqrt(sum(v * v for v in n)) or 1.0
    return (n[0] / ln, n[1] / ln, n[2] / ln)


class Model:
    """Faces in Trino coordinates (CCW seen from outside), one color each."""

    def __init__(self, name):
        self.name = name
        self.faces = []

    def poly(self, pts, rgb, out=None):
        pts = [tuple(p) for p in pts]
        if out is not None:
            n = _newell(pts)
            if n[0] * out[0] + n[1] * out[1] + n[2] * out[2] < 0:
                pts.reverse()
        self.faces.append((pts, rgb))

    def two_sided(self, pts, rgb, back=None):
        self.poly(pts, rgb)
        self.poly(list(reversed(pts)), back or shade(rgb, 0.8))

    def box(self, c, s, rgb, skip=(), top=1.08, bottom=0.7):
        """Axis-aligned box with simple baked light: top lighter, sides by
        facing, bottom darker. `skip`: faces never built ('-x', '+y', ...)."""
        x0, y0, z0 = (c[k] - s[k] / 2 for k in range(3))
        x1, y1, z1 = (c[k] + s[k] / 2 for k in range(3))
        faces = {
            "-x": ([(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)], 0.86),
            "+x": ([(x1, y0, z1), (x1, y0, z0), (x1, y1, z0), (x1, y1, z1)], 0.86),
            "-y": ([(x0, y0, z0), (x1, y0, z0), (x1, y0, z1), (x0, y0, z1)], bottom),
            "+y": ([(x0, y1, z1), (x1, y1, z1), (x1, y1, z0), (x0, y1, z0)], top),
            "-z": ([(x1, y0, z0), (x0, y0, z0), (x0, y1, z0), (x1, y1, z0)], 0.8),
            "+z": ([(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)], 0.97),
        }
        for k, (pts, light) in faces.items():
            if k not in skip:
                self.faces.append((pts, shade(rgb, light)))

    def taper(self, y0, y1, w0, d0, w1, d1, rgb, cx=0.0, cz=0.0, skip=(), sides=None):
        """A frustum between two rectangles (y0: w0 x d0, y1: w1 x d1)."""
        b = [(cx - w0 / 2, y0, cz - d0 / 2), (cx + w0 / 2, y0, cz - d0 / 2),
             (cx + w0 / 2, y0, cz + d0 / 2), (cx - w0 / 2, y0, cz + d0 / 2)]
        t = [(cx - w1 / 2, y1, cz - d1 / 2), (cx + w1 / 2, y1, cz - d1 / 2),
             (cx + w1 / 2, y1, cz + d1 / 2), (cx - w1 / 2, y1, cz + d1 / 2)]
        light = {"-z": 0.8, "+x": 0.88, "+z": 0.98, "-x": 0.88}
        names = ["-z", "+x", "+z", "-x"]
        for i in range(4):
            if names[i] in skip:
                continue
            col = (sides or {}).get(names[i], rgb)
            j = (i + 1) % 4
            self.poly([b[i], b[j], t[j], t[i]], shade(col, light[names[i]]),
                      out=(0, 0, -1) if names[i] == "-z" else (1, 0, 0) if names[i] == "+x"
                      else (0, 0, 1) if names[i] == "+z" else (-1, 0, 0))
        if "+y" not in skip:
            self.poly(t, shade(rgb, 1.08), out=(0, 1, 0))
        if "-y" not in skip:
            self.poly(b, shade(rgb, 0.7), out=(0, -1, 0))

    def disc(self, c, rx, rz, rgb, n=8, y=0.0):
        pts = [(c[0] + rx * math.cos(2 * math.pi * i / n), y, c[2] + rz * math.sin(2 * math.pi * i / n))
               for i in range(n)]
        self.poly(pts, rgb, out=(0, 1, 0))

    def build(self):
        me = bpy.data.meshes.new(self.name)
        bm = bmesh.new()
        col = bm.loops.layers.float_color.new("Col")
        for pts, rgb in self.faces:
            try:
                f = bm.faces.new([bm.verts.new(T(p)) for p in pts])
            except ValueError:
                continue
            lin = tuple(_lin(v) for v in rgb) + (1.0,)
            for loop in f.loops:
                loop[col] = lin
        bmesh.ops.remove_doubles(bm, verts=bm.verts[:], dist=1e-5)
        bmesh.ops.triangulate(bm, faces=bm.faces[:], quad_method="BEAUTY", ngon_method="BEAUTY")
        for f in bm.faces:
            f.smooth = False
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


# ------------------------------------------------------------------ pitch

HALF_LEN, HALF_WID = 52.5, 34.0
LINE_W = 0.22          # wider than 12 cm: lines must read at 240p
LIFT = 0.02
SURROUND_X, SURROUND_Z = 58.5, 40.0   # the tiles cover the run-off to the boards
TILES_X, TILES_Z = 6, 4
STRIPES = 12
GRASS_A, GRASS_B = (118, 186, 72), (92, 160, 58)
GRASS_OUT = (78, 140, 52)
LINE = (240, 244, 236)


def clip_poly(pts, x0, x1, z0, z1):
    """Sutherland–Hodgman of a convex polygon (x, z) against a rectangle."""
    def clip(poly, inside, cut):
        out = []
        for i in range(len(poly)):
            a, b = poly[i - 1], poly[i]
            ia, ib = inside(a), inside(b)
            if ib:
                if not ia:
                    out.append(cut(a, b))
                out.append(b)
            elif ia:
                out.append(cut(a, b))
        return out

    def at_x(xv):
        return lambda a, b: (xv, a[1] + (b[1] - a[1]) * (xv - a[0]) / (b[0] - a[0]))

    def at_z(zv):
        return lambda a, b: (a[0] + (b[0] - a[0]) * (zv - a[1]) / (b[1] - a[1]), zv)

    p = pts
    for inside, cut in ((lambda q: q[0] >= x0, at_x(x0)), (lambda q: q[0] <= x1, at_x(x1)),
                        (lambda q: q[1] >= z0, at_z(z0)), (lambda q: q[1] <= z1, at_z(z1))):
        if not p:
            return []
        p = clip(p, inside, cut)
    return p


def line_quads():
    """Every pitch marking as convex quads (x, z)."""
    w = LINE_W / 2
    q = []

    def rect(x0, z0, x1, z1):
        q.append([(x0, z0), (x1, z0), (x1, z1), (x0, z1)])

    # Touchlines, goal lines, halfway line.
    rect(-HALF_LEN - w, HALF_WID - w, HALF_LEN + w, HALF_WID + w)
    rect(-HALF_LEN - w, -HALF_WID - w, HALF_LEN + w, -HALF_WID + w)
    rect(-HALF_LEN - w, -HALF_WID, -HALF_LEN + w, HALF_WID)
    rect(HALF_LEN - w, -HALF_WID, HALF_LEN + w, HALF_WID)
    rect(-w, -HALF_WID, w, HALF_WID)
    for s in (-1, 1):
        gx = s * HALF_LEN
        for depth, half in ((16.5, 20.16), (5.5, 9.16)):
            ix = gx - s * depth
            rect(min(gx, ix), half - w, max(gx, ix), half + w)
            rect(min(gx, ix), -half - w, max(gx, ix), -half + w)
            rect(ix - w, -half, ix + w, half)
        # Penalty arc: the part of the 9.15 m circle around the spot outside
        # the area.
        spot = gx - s * 11.0
        edge = gx - s * 16.5
        n = 10
        a0 = math.acos(5.5 / 9.15)
        for i in range(n):
            t0 = -a0 + 2 * a0 * i / n
            t1 = -a0 + 2 * a0 * (i + 1) / n
            pts = []
            for t, r in ((t0, 9.15 - w), (t1, 9.15 - w), (t1, 9.15 + w), (t0, 9.15 + w)):
                pts.append((spot - s * r * math.cos(t), r * math.sin(t)))
            q.append(pts)
        # Spots.
        q.append([(spot + 0.18 * math.cos(2 * math.pi * k / 6), 0.18 * math.sin(2 * math.pi * k / 6))
                  for k in range(6)])
        # Corner arcs.
        for zs in (-1, 1):
            for i in range(4):
                t0, t1 = math.pi / 2 * i / 4, math.pi / 2 * (i + 1) / 4
                pts = []
                for t, r in ((t0, 1.0 - w), (t1, 1.0 - w), (t1, 1.0 + w), (t0, 1.0 + w)):
                    pts.append((gx - s * r * math.cos(t), zs * (HALF_WID - r * math.sin(t))))
                q.append(pts)
        _ = edge
    # Center circle and spot.
    n = 24
    for i in range(n):
        t0, t1 = 2 * math.pi * i / n, 2 * math.pi * (i + 1) / n
        pts = []
        for t, r in ((t0, 9.15 - w), (t1, 9.15 - w), (t1, 9.15 + w), (t0, 9.15 + w)):
            pts.append((r * math.cos(t), r * math.sin(t)))
        q.append(pts)
    q.append([(0.2 * math.cos(2 * math.pi * k / 6), 0.2 * math.sin(2 * math.pi * k / 6)) for k in range(6)])
    return q


def pitch_tile(ix, iz, fine=False):
    """Tile (ix, iz) of the 6 x 4 grid over the pitch and its run-off, or
    (`fine`) of the 12 x 8 grid: the game draws a coarse tile far from the
    camera and its four fine ones near it (a 20 m tile under the camera
    doesn't fit the N64 RSP's clip band and falls back to the CPU)."""
    nx, nz = (TILES_X * 2, TILES_Z * 2) if fine else (TILES_X, TILES_Z)
    tw = 2 * SURROUND_X / nx
    td = 2 * SURROUND_Z / nz
    x0, z0 = -SURROUND_X + ix * tw, -SURROUND_Z + iz * td
    x1, z1 = x0 + tw, z0 + td
    m = Model(f"pitch_{'f_' if fine else ''}{ix}_{iz}")
    # Turf: mowing stripes across the length inside the lines, darker grass
    # beyond them.
    sw = 2 * HALF_LEN / STRIPES
    bands = [(-SURROUND_X, -HALF_LEN, GRASS_OUT)]
    for k in range(STRIPES):
        bands.append((-HALF_LEN + k * sw, -HALF_LEN + (k + 1) * sw, GRASS_A if k % 2 == 0 else GRASS_B))
    bands.append((HALF_LEN, SURROUND_X, GRASS_OUT))
    for bx0, bx1, rgb in bands:
        for rz0, rz1, col in ((-SURROUND_Z, -HALF_WID, GRASS_OUT), (-HALF_WID, HALF_WID, rgb),
                              (HALF_WID, SURROUND_Z, GRASS_OUT)):
            cx0, cx1 = max(bx0, x0), min(bx1, x1)
            cz0, cz1 = max(rz0, z0), min(rz1, z1)
            if cx1 - cx0 > 1e-4 and cz1 - cz0 > 1e-4:
                m.poly([(cx0, 0, cz0), (cx1, 0, cz0), (cx1, 0, cz1), (cx0, 0, cz1)], col, out=(0, 1, 0))
    for quad in line_quads():
        c = clip_poly(quad, x0, x1, z0, z1)
        if len(c) >= 3:
            m.poly([(x, LIFT, z) for x, z in c], LINE, out=(0, 1, 0))
    return m


# ------------------------------------------------------------- stadium

def ad_boards(variant):
    """10 m of advertising boards (0.9 m high, facing +z = the pitch), two
    color schemes."""
    schemes = [[(200, 30, 40), (240, 240, 240), (30, 70, 190), (250, 200, 40)],
               [(30, 70, 190), (250, 200, 40), (200, 30, 40), (40, 160, 90)]]
    m = Model(f"boards_{variant}")
    cols = schemes[variant]
    for i in range(4):
        x0 = -5 + i * 2.5
        c = cols[i % len(cols)]
        m.poly([(x0, 0, 0), (x0 + 2.5, 0, 0), (x0 + 2.5, 0.9, 0), (x0, 0.9, 0)], c, out=(0, 0, 1))
        # A light "logo" stripe.
        m.poly([(x0 + 0.4, 0.35, 0.01), (x0 + 2.1, 0.35, 0.01), (x0 + 2.1, 0.55, 0.01), (x0 + 0.4, 0.55, 0.01)],
               shade(c, 1.5) if sum(c) < 500 else shade(c, 0.6), out=(0, 0, 1))
    m.poly([(-5, 0.9, 0), (5, 0.9, 0), (5, 0.9, -0.15), (-5, 0.9, -0.15)], (40, 40, 48), out=(0, 1, 0))
    return m


def crowd_colors(rng, home):
    pal = [home, home, (240, 240, 240), (60, 60, 70), (230, 200, 60), (40, 90, 200), (210, 40, 50),
           (240, 150, 50), (120, 60, 160), (60, 160, 90)]
    return pal[rng.randrange(len(pal))]


def stand_section(name, width, rows, rng_seed, home, dense):
    """One section of terracing facing +z (the pitch), `width` meters wide:
    a front wall, stepped rows with the crowd, a back wall and a roof lip.
    Console: the crowd is a mosaic of colored blocks on each riser; PC
    (`dense`): one little person (body + head) per seat."""
    rng = random.Random(rng_seed)
    m = Model(name)
    concrete, step_col = (150, 152, 160), (110, 112, 122)
    rise, tread = 0.5, 0.85
    w2 = width / 2
    # Advertising boards at the pitch's edge, 2 m in front of the stand
    # (part of the section: no extra draw), 0.9 m high, in 5 m panels.
    schemes = [(200, 30, 40), (240, 240, 240), (30, 70, 190), (250, 200, 40), (40, 160, 90)]
    panels = int(round(width / 5.0))
    for k in range(panels):
        x0 = -w2 + k * width / panels
        x1 = x0 + width / panels
        c = schemes[(rng_seed + k) % len(schemes)]
        m.poly([(x0, 0, 2.0), (x1, 0, 2.0), (x1, 0.9, 2.0), (x0, 0.9, 2.0)], c, out=(0, 0, 1))
        stripe = shade(c, 1.6) if sum(c) < 500 else shade(c, 0.55)
        m.poly([(x0 + 0.6, 0.35, 2.01), (x1 - 0.6, 0.35, 2.01), (x1 - 0.6, 0.55, 2.01), (x0 + 0.6, 0.55, 2.01)],
               stripe, out=(0, 0, 1))
    m.poly([(-w2, 0.9, 2.0), (w2, 0.9, 2.0), (w2, 0.9, 1.85), (-w2, 0.9, 1.85)], (40, 40, 48), out=(0, 1, 0))
    # Front wall (behind the boards' moat).
    m.poly([(-w2, 0, 0), (w2, 0, 0), (w2, 1.6, 0), (-w2, 1.6, 0)], (60, 64, 76), out=(0, 0, 1))
    y, z = 1.6, 0.0
    for r in range(rows):
        # Tread (top) and riser (front) of the row.
        m.poly([(-w2, y, z), (w2, y, z), (w2, y, z - tread), (-w2, y, z - tread)], step_col, out=(0, 1, 0))
        if dense:
            seats = int(width / 0.55)
            for s in range(seats):
                if rng.random() < 0.12:
                    continue
                x = -w2 + (s + 0.5) * width / seats + rng.uniform(-0.08, 0.08)
                c = crowd_colors(rng, home)
                h = rng.uniform(0.5, 0.62)
                zz = z - tread * 0.45
                m.box((x, y + h / 2, zz), (0.42, h, 0.3), c, skip=("-y", "-z"))
                skin = rng.choice([(240, 200, 160), (200, 150, 110), (150, 100, 70), (100, 65, 45)])
                m.box((x, y + h + 0.11, zz), (0.2, 0.22, 0.2), skin, skip=("-y", "-z"))
        else:
            blocks = max(2, int(width / 4.5))
            for b in range(blocks):
                x0 = -w2 + b * width / blocks
                x1 = x0 + width / blocks
                c = crowd_colors(rng, home)
                # People as a colored band on the next riser, a little lighter
                # at the top (heads).
                m.poly([(x0, y, z - tread), (x1, y, z - tread), (x1, y + rise, z - tread),
                        (x0, y + rise, z - tread)], c, out=(0, 0, 1))
        y += rise
        z -= tread
        if not dense:
            continue
        m.poly([(-w2, y - rise, z), (w2, y - rise, z), (w2, y, z), (-w2, y, z)], concrete, out=(0, 0, 1))
    # Back wall and roof lip.
    m.poly([(-w2, y, z), (w2, y, z), (w2, y + 3.0, z), (-w2, y + 3.0, z)], (90, 94, 108), out=(0, 0, 1))
    m.poly([(-w2, y + 3.0, z), (w2, y + 3.0, z), (w2, y + 3.4, z + 6.0), (-w2, y + 3.4, z + 6.0)],
           (180, 184, 196), out=(0, -1, 0))
    m.poly([(-w2, y + 3.4, z + 6.0), (w2, y + 3.4, z + 6.0), (w2, y + 3.0, z), (-w2, y + 3.0, z)],
           (210, 214, 226), out=(0, 1, 0))
    return m


def flood_tower():
    m = Model("flood_tower")
    m.box((0, 14, 0), (0.6, 28, 0.6), (120, 124, 136), skip=("-y",))
    m.box((0, 28.8, 0.4), (4.0, 2.2, 0.4), (70, 74, 84), skip=("-y",))
    for i in range(4):
        for j in range(2):
            m.poly([(-1.8 + i * 0.95, 28.0 + j * 1.0, 0.61), (-1.0 + i * 0.95, 28.0 + j * 1.0, 0.61),
                    (-1.0 + i * 0.95, 28.8 + j * 1.0, 0.61), (-1.8 + i * 0.95, 28.8 + j * 1.0, 0.61)],
                   (255, 250, 220), out=(0, 0, 1))
    return m


def goal(dense):
    """The goal with its opening facing +z: posts and bar (12 cm, white),
    the net 2 m deep, every panel two-sided (built as two faces)."""
    m = Model("goal")
    w2, h, d, r = 3.66, 2.44, 2.0, 0.06
    white = (245, 245, 245)
    m.box((-w2 - r, h / 2, 0), (2 * r, h, 2 * r), white, skip=("-y",))
    m.box((w2 + r, h / 2, 0), (2 * r, h, 2 * r), white, skip=("-y",))
    m.box((0, h + r, 0), (2 * (w2 + 2 * r), 2 * r, 2 * r), white)
    net, net2 = (226, 228, 232), (196, 200, 206)
    cols = 6 if dense else 1
    for c in range(cols):
        xa = -w2 + 2 * w2 * c / cols
        xb = -w2 + 2 * w2 * (c + 1) / cols
        col = net if c % 2 == 0 else net2
        # Back (sloping a little) and roof.
        m.two_sided([(xa, 0, -d), (xb, 0, -d), (xb, h * 0.8, -d * 0.9), (xa, h * 0.8, -d * 0.9)], col)
        m.two_sided([(xa, h * 0.8, -d * 0.9), (xb, h * 0.8, -d * 0.9), (xb, h, 0), (xa, h, 0)], shade(col, 1.05))
    for s in (-1, 1):
        m.two_sided([(s * w2, 0, 0), (s * w2, 0, -d), (s * w2, h * 0.8, -d * 0.9), (s * w2, h, 0)], net2)
    # Back bars on the ground.
    m.box((0, 0.03, -d), (2 * w2, 0.06, 0.06), (200, 200, 205), skip=("-y",))
    return m


def corner_flag():
    m = Model("corner_flag")
    m.box((0, 0.75, 0), (0.05, 1.5, 0.05), (245, 245, 245), skip=("-y",))
    m.two_sided([(0.03, 1.5, 0), (0.45, 1.38, 0), (0.03, 1.22, 0)], (240, 60, 40))
    return m


def ball(detail):
    """Icosahedron (console: 20 faces) or one subdivision (PC: 80), black
    faces around 12 'pentagons' in the classic pattern."""
    t = (1 + 5 ** 0.5) / 2
    v = [(-1, t, 0), (1, t, 0), (-1, -t, 0), (1, -t, 0), (0, -1, t), (0, 1, t), (0, -1, -t), (0, 1, -t),
         (t, 0, -1), (t, 0, 1), (-t, 0, -1), (-t, 0, 1)]
    f = [(0, 11, 5), (0, 5, 1), (0, 1, 7), (0, 7, 10), (0, 10, 11), (1, 5, 9), (5, 11, 4), (11, 10, 2),
         (10, 7, 6), (7, 1, 8), (3, 9, 4), (3, 4, 2), (3, 2, 6), (3, 6, 8), (3, 8, 9), (4, 9, 5), (2, 4, 11),
         (6, 2, 10), (8, 6, 7), (9, 8, 1)]
    R = 0.11

    def norm(p):
        ln = math.sqrt(sum(c * c for c in p))
        return tuple(c / ln for c in p)

    verts = [norm(p) for p in v]
    faces = [tuple(verts[i] for i in tri) for tri in f]
    if detail:
        out = []
        for a, b, c in faces:
            ab = norm(tuple((a[k] + b[k]) / 2 for k in range(3)))
            bc = norm(tuple((b[k] + c[k]) / 2 for k in range(3)))
            ca = norm(tuple((c[k] + a[k]) / 2 for k in range(3)))
            out += [(a, ab, ca), (ab, b, bc), (ca, bc, c), (ab, bc, ca)]
        faces = out
    m = Model("ball")
    for tri in faces:
        c = tuple(sum(p[k] for p in tri) / 3 for k in range(3))
        # Near an original vertex (the 12 pentagon centers): black.
        near = max(sum(c[k] * vv[k] for k in range(3)) for vv in verts)
        dark = near > (0.96 if detail else 0.99)
        if not detail:
            dark = tri in faces[::4]
        col = (35, 35, 40) if dark else (248, 248, 248)
        m.poly([tuple(p[k] * R for k in range(3)) for p in tri], col, out=c)
    return m


# --------------------------------------------------------------- players

SKIN = (236, 236, 236)    # tinted by skin tone
KIT = (236, 236, 236)     # tinted by kit colors
BOOT = (46, 46, 52)
HAIR = (70, 56, 46)
# The console heads' baked hair: dark, light (blond and red hair under the
# skin tint), or none (shaved: the crown a shade of the skin).
CONSOLE_HAIR = {"head": HAIR, "head_light": (214, 176, 112), "head_shaved": None}
EYE = (30, 26, 26)


def body_part(name, base=None, boot=BOOT, hair=HAIR):
    """A body part from the shared spec (`solid_eleven_anim.BODY`): its
    solids, then flat details. Kit and skin regions are light grey for the
    per-draw tint (or `base`, baked); boots and hair keep their own
    colors."""
    joint, tint, tiers, budget, solids = anim.BODY[name]
    if base is None:
        base = SKIN if tint == "skin" else KIT
    colors = {"base": base, "hair": hair, "boot": boot}
    m = Model(name[:-2] if name.endswith("_x") else name)
    for so in solids:
        rgb = colors[so["color"]]
        skip = tuple(so["skip"])
        sides = None
        top = None
        if name == "head":
            # The console head is one wedge: hair on top and at the back
            # (or a shaved crown).
            top = shade(hair, 1.1) if hair else shade(SKIN, 0.94)
            m.taper(so["y0"], so["y1"], so["w0"], so["d0"], so["w1"], so["d1"], rgb, so["cx"], so["cz"],
                    skip=skip + ("+y",), sides={"-z": hair} if hair else None)
        elif name == "torso" and so is solids[-1]:
            # The console shirt's top is its darker collar.
            top = shade(KIT, 0.62)
            m.taper(so["y0"], so["y1"], so["w0"], so["d0"], so["w1"], so["d1"], rgb, so["cx"], so["cz"],
                    skip=skip + ("+y",))
        if top is not None:
            x0, x1, z0, z1 = so["cx"] - so["w1"] / 2, so["cx"] + so["w1"] / 2, so["cz"] - so["d1"] / 2, so["cz"] + so["d1"] / 2
            m.poly([(x0, so["y1"], z1), (x1, so["y1"], z1), (x1, so["y1"], z0), (x0, so["y1"], z0)], top, out=(0, 1, 0))
            continue
        m.taper(so["y0"], so["y1"], so["w0"], so["d0"], so["w1"], so["d1"], rgb, so["cx"], so["cz"], skip=skip)
    face_z = lambda so, y: so["cz"] + (so["d0"] + (so["d1"] - so["d0"]) * (y - so["y0"]) / (so["y1"] - so["y0"])) / 2
    if name in ("head", "head_hd"):
        face = solids[0] if name == "head" else solids[2]
        ey = 0.25 if name == "head" else 0.265
        z = face_z(face, ey) + 0.002
        for x in (-0.055, 0.055):
            m.poly([(x - 0.025, ey - 0.013, z), (x + 0.025, ey - 0.013, z), (x + 0.025, ey + 0.012, z),
                    (x - 0.025, ey + 0.012, z)], EYE, out=(0, 0, 1))
        if name == "head_hd":
            # The nose, from the face (the jaw's front) out.
            m.taper(0.19, 0.245, 0.04, 0.044, 0.03, 0.016, shade(SKIN, 0.92), cz=0.122, skip=("-z",))
            # The brows, a little darker than the skin (any skin tone).
            zb = face_z(face, 0.287) + 0.002
            for x in (-0.055, 0.055):
                m.poly([(x - 0.032, 0.283, zb), (x + 0.032, 0.283, zb), (x + 0.032, 0.291, zb),
                        (x - 0.032, 0.291, zb)], shade(SKIN, 0.55), out=(0, 0, 1))
    elif name == "torso_hd":
        top = solids[2]
        m.box((0, top["y1"] + 0.006, 0.0), (0.16, 0.012, 0.12), shade(KIT, 0.62), skip=("-y",))
        zb = -face_z(solids[1], 0.25) - 0.002
        m.poly([(-0.08, 0.17, zb), (0.08, 0.17, zb), (0.08, 0.29, zb), (-0.08, 0.29, zb)],
               shade(KIT, 0.7), out=(0, 0, -1))
        zf = face_z(solids[1], 0.25) + 0.002
        m.poly([(0.06, 0.22, zf), (0.11, 0.22, zf), (0.11, 0.27, zf), (0.06, 0.27, zf)],
               shade(KIT, 0.7), out=(0, 0, 1))
    elif name == "shin_x":
        so = solids[1]
        m.box((0, so["y1"] - 0.015, 0), (so["w1"] + 0.006, 0.03, so["d1"] + 0.006), shade(KIT, 0.72),
              skip=("+y", "-y"))
    return m


def console_head(name):
    """A console head: the wedge with its hair baked (`CONSOLE_HAIR`)."""
    m = body_part("head", hair=CONSOLE_HAIR[name])
    m.name = name
    return m


def hair_part(style):
    """A PC hair style (`solid_eleven_anim.HAIR`): light grey for the
    per-draw hair color, the shell's sides a shade darker than its crown."""
    budget, solids = anim.HAIR[style]
    m = Model("hair_" + style)
    for so in solids:
        m.taper(so["y0"], so["y1"], so["w0"], so["d0"], so["w1"], so["d1"], KIT, so["cx"], so["cz"],
                skip=tuple(so["skip"]))
    return m


def leg_kits():
    """(index, shorts, socks) of examples/solid-eleven/assets/leg_kits.txt."""
    out = []
    with open(os.path.join(GAME, "assets", "leg_kits.txt"), encoding="utf-8") as f:
        for line in f:
            if line.startswith("#") or not line.strip():
                continue
            v = [int(x) for x in line.split()]
            out.append((v[0], tuple(v[1:4]), tuple(v[4:7])))
    return out


def leg_kit(shorts, socks):
    """A console leg in one mesh: the shorts leg and the sock with its
    boot, colors baked (no per-draw tint: one draw instead of two)."""
    m = Model("leg_kit")
    m.faces += body_part("shorts_leg_x", base=shorts).faces
    boot = tuple(int(round(b * s / 255)) for b, s in zip(BOOT, socks))
    m.faces += body_part("leg_x", base=socks, boot=boot).faces
    return m


GOLD = (236, 190, 64)


def trophy():
    """The league trophy: a gold cup with two handles on a dark plinth,
    0.5 m tall, its foot at the origin (the captain's hands hold it
    there). Fixed colors (never tinted)."""
    m = Model("trophy")
    wood = (70, 44, 30)
    m.taper(-0.08, 0.0, 0.17, 0.17, 0.15, 0.15, wood)
    m.taper(0.0, 0.05, 0.12, 0.12, 0.06, 0.06, GOLD, skip=("-y",))
    m.taper(0.05, 0.16, 0.04, 0.04, 0.04, 0.04, shade(GOLD, 0.9), skip=("-y", "+y"))
    m.taper(0.16, 0.2, 0.04, 0.04, 0.09, 0.09, GOLD, skip=("-y",))
    m.taper(0.2, 0.42, 0.09, 0.09, 0.22, 0.22, GOLD, skip=("-y", "+y"))
    # The bowl's mouth: a darker inside.
    m.poly([(-0.11, 0.42, 0.11), (0.11, 0.42, 0.11), (0.11, 0.42, -0.11), (-0.11, 0.42, -0.11)],
           shade(GOLD, 0.55), out=(0, 1, 0))
    # The handles: a bar each side, joined to the bowl at both ends.
    for sx in (-1, 1):
        inner = "-x" if sx > 0 else "+x"
        m.box((sx * 0.15, 0.33, 0), (0.03, 0.15, 0.03), shade(GOLD, 1.05))
        m.box((sx * 0.125, 0.395, 0), (0.05, 0.02, 0.025), GOLD, skip=(inner,))
        m.box((sx * 0.112, 0.265, 0), (0.075, 0.02, 0.025), GOLD, skip=(inner,))
    return m


def flag_hand():
    """The linesman's flag, held at the hand (drawn at the forearm's end)."""
    m = Model("flag_hand")
    m.box((0, 0.2, 0), (0.025, 0.55, 0.025), (60, 60, 60))
    m.two_sided([(0.0, 0.47, 0.0), (0.0, 0.47, 0.32), (0.0, 0.25, 0.32), (0.0, 0.25, 0.0)], (250, 210, 30))
    return m


def card():
    """The referee's card, white (the game tints it yellow or red): held
    at the hand (drawn at the arm's end like the linesman's flag), it
    stands out of the fist along the arm, turned 40 degrees about the arm
    so both the player and the broadcast camera see its face."""
    m = Model("card")
    # Larger than life (a real one is 8 x 12 cm), so it reads at the
    # broadcast distance.
    w, h, d = 0.16, 0.22, 0.02
    y0, y1 = -0.015 - h, -0.015
    a = math.radians(40)
    rot = lambda x, z: (x * math.cos(a) + z * math.sin(a), -x * math.sin(a) + z * math.cos(a))

    def p(x, y, z):
        rx, rz = rot(x, z)
        return (rx, y, rz)

    xs, zs = (-w / 2, w / 2), (-d / 2, d / 2)
    faces = [
        ([p(xs[0], y0, zs[1]), p(xs[1], y0, zs[1]), p(xs[1], y1, zs[1]), p(xs[0], y1, zs[1])], (0, 0, 1), 0.97),
        ([p(xs[1], y0, zs[0]), p(xs[0], y0, zs[0]), p(xs[0], y1, zs[0]), p(xs[1], y1, zs[0])], (0, 0, -1), 0.9),
        ([p(xs[1], y0, zs[1]), p(xs[1], y0, zs[0]), p(xs[1], y1, zs[0]), p(xs[1], y1, zs[1])], (1, 0, 0), 0.86),
        ([p(xs[0], y0, zs[0]), p(xs[0], y0, zs[1]), p(xs[0], y1, zs[1]), p(xs[0], y1, zs[0])], (-1, 0, 0), 0.86),
        ([p(xs[0], y1, zs[1]), p(xs[1], y1, zs[1]), p(xs[1], y1, zs[0]), p(xs[0], y1, zs[0])], (0, 1, 0), 1.0),
        ([p(xs[0], y0, zs[0]), p(xs[1], y0, zs[0]), p(xs[1], y0, zs[1]), p(xs[0], y0, zs[1])], (0, -1, 0), 0.8),
    ]
    for pts, n, light in faces:
        rx, rz = rot(n[0], n[2])
        m.poly(pts, shade((255, 255, 255), light), out=(rx, n[1], rz))
    return m


# ---------------------------------------------------------------- markers

def ring(name, r0, r1, n, rgb):
    m = Model(name)
    for i in range(n):
        t0, t1 = 2 * math.pi * i / n, 2 * math.pi * (i + 1) / n
        m.poly([(r0 * math.cos(t0), 0, r0 * math.sin(t0)), (r0 * math.cos(t1), 0, r0 * math.sin(t1)),
                (r1 * math.cos(t1), 0, r1 * math.sin(t1)), (r1 * math.cos(t0), 0, r1 * math.sin(t0))],
               rgb, out=(0, 1, 0))
    return m


def arrow():
    """The P1 marker: a downward pyramid floating over the player."""
    m = Model("arrow")
    tip = (0, 0, 0)
    base = [(-0.16, 0.26, -0.16), (0.16, 0.26, -0.16), (0.16, 0.26, 0.16), (-0.16, 0.26, 0.16)]
    for a, b in zip(base, base[1:] + base[:1]):
        m.poly([a, b, tip], (240, 240, 240), out=((a[0] + b[0]) / 2, -0.2, (a[2] + b[2]) / 2))
    m.poly(base, (255, 255, 255), out=(0, 1, 0))
    return m


def dash():
    """A pass-lane dash: 1 m long, 18 cm wide, along +z."""
    m = Model("dash")
    m.poly([(-0.09, 0, 0), (0.09, 0, 0), (0.09, 0, 1.0), (-0.09, 0, 1.0)], (255, 255, 255), out=(0, 1, 0))
    return m


def dot():
    """A trajectory bead: an octahedron."""
    m = Model("dot")
    r = 0.14
    pts = [(r, 0, 0), (0, 0, r), (-r, 0, 0), (0, 0, -r)]
    for a, b in zip(pts, pts[1:] + pts[:1]):
        for y in (r, -r):
            tri = [a, b, (0, y, 0)]
            m.poly(tri, (255, 255, 255), out=((a[0] + b[0]) / 2, y, (a[2] + b[2]) / 2))
    return m


def pedestal():
    """The menus' glowing turntable (reference art): a dark disc with a
    bright rim ring; the game tints the ring."""
    m = Model("pedestal")
    n = 20
    top, rim = 0.12, 0.9
    for i in range(n):
        t0, t1 = 2 * math.pi * i / n, 2 * math.pi * (i + 1) / n
        p = lambda t, r, y: (r * math.cos(t), y, r * math.sin(t))
        m.poly([p(t0, rim, top), p(t1, rim, top), (0, top, 0)], (40, 46, 70), out=(0, 1, 0))
        m.poly([p(t0, rim, top), p(t0, rim + 0.08, 0), p(t1, rim + 0.08, 0), p(t1, rim, top)], (90, 220, 255),
               out=(math.cos(t0), 0.3, math.sin(t0)))
    return m


# ----------------------------------------------------------------- rig

def player_rig():
    """The PC parts posed by the skeleton of solid_eleven_anim.py, one object
    per joint (rigid parts), every clip as an NLA track: the animation QA
    model (`model inspect --anim`, the editor's Animation tab)."""
    # Every PC part on a joint, one mesh per joint.
    builders = {}
    for name, (joint, tint, tiers, budget, solids) in anim.BODY.items():
        if "pc" not in tiers.split():
            continue
        sides = [joint] + ([joint[:-2] + "_r"] if name.endswith("_x") else [])
        for j in sides:
            builders.setdefault(j, []).append(name)

    def build_joint(j):
        m = Model("rig_" + j)
        for n in builders[j]:
            m.faces += body_part(n).faces
        if j == "head":
            m.faces += hair_part("crop").faces
        return m

    objs = {}
    for name, parent, piv in anim.JOINTS:
        mdl = build_joint(name)
        ob = mdl.build()
        ob.location = T(piv)
        if parent is not None:
            ob.parent = objs[parent]
        objs[name] = ob
    bpy.context.view_layer.update()
    names = list(objs)
    fps = 24
    from mathutils import Quaternion, Vector
    for clip, (secs, loop, keys) in anim.CLIPS.items():
        acts = {}
        for n in names:
            o = objs[n]
            act = bpy.data.actions.new(f"{clip}__{o.name}")
            act.use_fake_user = True
            o.animation_data_create()
            o.animation_data.action = act
            try:
                o.animation_data.action_slot = act.slots[0] if act.slots else o.animation_data.action_slot
            except Exception:
                pass
            acts[n] = act
        frames = list(keys)
        if loop:
            frames.append((1.0, keys[0][1], keys[0][2]))
        for t, pose, root in frames:
            f = 1 + round(t * secs * fps)
            for n in names:
                o = objs[n]
                rx, ry, rz = pose.get(n, (0, 0, 0))
                qx, qy, qz, qw = anim.quat_from_euler(rx, ry, rz)
                # Trino (x, y, z) -> Blender (x, -z, y) for a rotation axis.
                o.rotation_mode = 'QUATERNION'
                o.rotation_quaternion = Quaternion((qw, qx, -qz, qy))
                o.keyframe_insert("rotation_quaternion", frame=f)
                if n == "pelvis":
                    piv = anim.JOINTS[0][2]
                    g = anim.grounded_root(pose, root, "pc")
                    o.location = Vector(T((piv[0] + g[0], piv[1] + g[1], piv[2] + g[2])))
                    o.keyframe_insert("location", frame=f)
        for n in names:
            o = objs[n]
            o.animation_data.action = None
            tr = o.animation_data.nla_tracks.new()
            tr.name = clip
            st = tr.strips.new(clip, 1, acts[n])
            st.name = clip
            tr.mute = True
    for n in names:
        objs[n].rotation_quaternion = (1, 0, 0, 0)
    objs["pelvis"].location = T(anim.JOINTS[0][2])
    return objs["pelvis"]


# ----------------------------------------------------------------- table

# name -> (builder, out dir, max triangles, QA framing (target, ortho))
MODELS = {}


def reg(name, build, where, budget, frame):
    MODELS[name] = (build, where, budget, frame)


for ix in range(TILES_X):
    for iz in range(TILES_Z):
        reg(f"pitch_{ix}_{iz}", (lambda ix=ix, iz=iz: pitch_tile(ix, iz)), SHARED, 140,
            ((-SURROUND_X + (ix + 0.5) * 2 * SURROUND_X / TILES_X, (SURROUND_Z - (iz + 0.5) * 2 * SURROUND_Z / TILES_Z), 0), 22))
for ix in range(TILES_X * 2):
    for iz in range(TILES_Z * 2):
        reg(f"pitch_f_{ix}_{iz}", (lambda ix=ix, iz=iz: pitch_tile(ix, iz, True)), SHARED, 80,
            ((-SURROUND_X + (ix + 0.5) * SURROUND_X / TILES_X, (SURROUND_Z - (iz + 0.5) * SURROUND_Z / TILES_Z), 0), 11))
reg("stand_long_a", lambda: stand_section("stand_long_a", 19.5, 7, 1, (206, 32, 41), False), SHARED, 100, ((0, 2, 4), 22))
reg("stand_long_b", lambda: stand_section("stand_long_b", 19.5, 7, 2, (28, 90, 200), False), SHARED, 100, ((0, 2, 4), 22))
reg("stand_end_a", lambda: stand_section("stand_end_a", 20.0, 6, 3, (206, 32, 41), False), SHARED, 90, ((0, 2, 4), 22))
reg("stand_end_b", lambda: stand_section("stand_end_b", 20.0, 6, 4, (28, 90, 200), False), SHARED, 90, ((0, 2, 4), 22))
reg("stand_long_a@pc", lambda: stand_section("stand_long_a", 19.5, 12, 1, (206, 32, 41), True), PC, 12000, ((0, 2, 4), 22))
reg("stand_long_b@pc", lambda: stand_section("stand_long_b", 19.5, 12, 2, (28, 90, 200), True), PC, 12000, ((0, 2, 4), 22))
reg("stand_end_a@pc", lambda: stand_section("stand_end_a", 20.0, 10, 3, (206, 32, 41), True), PC, 12000, ((0, 2, 4), 22))
reg("stand_end_b@pc", lambda: stand_section("stand_end_b", 20.0, 10, 4, (28, 90, 200), True), PC, 12000, ((0, 2, 4), 22))
reg("flood_tower", flood_tower, SHARED, 60, ((0, 0, 15), 32))
reg("goal", lambda: goal(False), SHARED, 70, ((0, -1, 1.2), 9))
reg("goal@pc", lambda: goal(True), PC, 160, ((0, -1, 1.2), 9))
reg("corner_flag", corner_flag, SHARED, 16, ((0, 0, 0.8), 2))
reg("ball", lambda: ball(False), SHARED, 20, ((0, 0, 0), 0.3))
reg("ball@pc", lambda: ball(True), PC, 80, ((0, 0, 0), 0.3))
for _name, (_joint, _tint, _tiers, _budget, _solids) in anim.BODY.items():
    _file = _name[:-2] if _name.endswith("_x") else _name
    _where = PC if _tiers.split() == ["pc"] else SHARED
    _y = (_solids[0]["y0"] + _solids[0]["y1"]) / 2
    reg(_file, (lambda n=_name: body_part(n)), _where, _budget, ((0, 0, _y), 0.8))
for _style, (_budget, _solids) in anim.HAIR.items():
    reg(f"hair_{_style}", (lambda st=_style: hair_part(st)), PC, _budget, ((0, 0, 0.33), 0.6))
for _name in ("head_light", "head_shaved"):
    reg(_name, (lambda n=_name: console_head(n)), SHARED, anim.BODY["head"][3], ((0, 0, 0.17), 0.8))
reg("trophy", trophy, SHARED, 120, ((0, 0, 0.2), 0.7))
for _i, _shorts, _socks in leg_kits():
    reg(f"leg_k{_i:02}", (lambda a=_shorts, b=_socks: leg_kit(a, b)), SHARED,
        anim.BODY["shorts_leg_x"][3] + anim.BODY["leg_x"][3], ((0, 0, -0.45), 1.1))

reg("flag_hand", flag_hand, SHARED, 18, ((0, 0, 0.25), 0.8))
reg("card", card, SHARED, 12, ((0, 0, -0.07), 0.3))
reg("ring", lambda: ring("ring", 0.55, 0.68, 16, (255, 255, 255)), SHARED, 32, ((0, 0, 0), 1.6))
reg("ring_small", lambda: ring("ring_small", 0.4, 0.48, 10, (255, 255, 255)), SHARED, 20, ((0, 0, 0), 1.2))
reg("arrow", arrow, SHARED, 8, ((0, 0, 0.1), 0.6))
reg("dash", dash, SHARED, 2, ((0, 0, 0), 1.2))
reg("dot", dot, SHARED, 8, ((0, 0, 0), 0.4))
reg("pedestal", pedestal, SHARED, 60, ((0, 0, 0), 2.2))
reg("player_rig", player_rig, PC, 400, ((0, 0, 0.9), 2.4))


def main():
    argv = sys.argv[sys.argv.index("--") + 1:] if "--" in sys.argv else []
    only = None
    if "--only" in argv:
        only = set(argv[argv.index("--only") + 1].split(","))
    render = "--no-render" not in argv
    qa = os.path.join(REPO, "target", "blender_solid_eleven")
    os.makedirs(qa, exist_ok=True)
    global ns
    ns = {"__name__": "trino_tools", "SCRATCH": qa, "REPO": REPO}
    for f in ("qa_render.py", "color_tools.py", "rig_tools.py", "trino_export.py"):
        exec(open(os.path.join(REPO, "tools", "blender", f), encoding="utf-8").read(), ns)

    report = {"blender": bpy.app.version_string, "models": {}}
    ok = True
    for key, (build, where, budget, (target, ortho)) in MODELS.items():
        # "x@pc" is the PC's high-detail variant: its own PC-only asset
        # (`x_hd`), so the PC's N64 and 3DS profiles still draw the console
        # model.
        name = key.split("@")[0] + ("_hd" if key.endswith("@pc") else "")
        # `--only legs`: every kit leg.
        if only and name not in only and key not in only and not ("legs" in only and name.startswith("leg_k")):
            continue
        for o in list(bpy.data.objects):
            bpy.data.objects.remove(o, do_unlink=True)
        for a in list(bpy.data.actions):
            bpy.data.actions.remove(a)
        try:
            ob = build()
            if isinstance(ob, Model):
                ob = ob.build()
            path = os.path.join(where, name + ".glb")
            animated = name == "player_rig"
            ns["export_glb"]([ob.name], path, animations=animated)
            ins = ns["inspect"](path, fps=15 if animated else None,
                                once=[c for c, (_, lp, _) in anim.CLIPS.items() if not lp] if animated else None)
            tris = ins.get("triangles", 1 << 30)
            good = bool(ins.get("ok")) and tris <= budget
            report["models"][key] = {"ok": good, "triangles": tris, "budget": budget,
                                     "size": ins.get("size"), "warnings": ins.get("warnings")}
            ok &= good
            if render and not name.startswith("pitch_") or (render and name in ("pitch_2_1", "pitch_f_5_3")):
                ns["vertex_color_mode"](True)
                tiles = [ns["shot"](os.path.join(qa, f"{key}_{az}_{el}.png"), az, el, target, ortho, res=240)
                         for az, el in ((0, 10), (45, 25), (90, 10), (180, 25), (0, 80), (30, -40))]
                ns["sheet"](os.path.join(qa, f"{key.replace('@', '_')}.png"), tiles, 3)
        except Exception:
            ok = False
            report["models"][key] = {"ok": False, "error": traceback.format_exc()}
    # The game's pose tables come from the same clip table as the rig.
    clips = os.path.join(GAME, "src", "render", "clips_gen.rs")
    with open(clips, "w") as f:
        f.write(anim.rust_source())
    report["clips"] = {"path": clips, "clips": len(anim.CLIPS), "joints": len(anim.NAMES)}
    report["ok"] = ok
    print("TRINO_BLENDER_MODELS " + json.dumps(report))
    sys.stdout.flush()
    if not ok:
        sys.exit(1)


main()
