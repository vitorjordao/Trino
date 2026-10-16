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
#   tools/blender/solid_eleven_anim.py): consoles draw 7 parts (head, torso,
#   shorts with the blob shadow, two arms, two legs), the PC 11 (elbows and
#   knees) with higher-detail overrides.
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
EYE = (30, 26, 26)


def part_head(detail):
    """Head on the neck pivot (0, 0, 0): neck, head, hair cap, eyes, nose."""
    m = Model("head")
    m.box((0, 0.05, 0), (0.11, 0.1, 0.11), SKIN, skip=("-y", "+y"))
    if detail:
        m.taper(0.08, 0.20, 0.2, 0.22, 0.22, 0.24, SKIN, skip=("-y", "+y"))
        m.taper(0.20, 0.31, 0.22, 0.24, 0.18, 0.21, SKIN, skip=("-y",))
        m.taper(0.24, 0.335, 0.235, 0.255, 0.19, 0.2, HAIR, cz=-0.01, skip=("-y", "+z"))
        m.box((0, 0.19, -0.115), (0.22, 0.18, 0.04), HAIR, skip=("+z",))
        for x in (-0.055, 0.055):
            m.poly([(x - 0.025, 0.205, 0.122), (x + 0.025, 0.205, 0.122), (x + 0.025, 0.225, 0.122),
                    (x - 0.025, 0.225, 0.122)], EYE, out=(0, 0, 1))
        m.taper(0.15, 0.2, 0.04, 0.05, 0.03, 0.01, shade(SKIN, 0.92), cz=0.13, skip=("-z",))
    else:
        # One box: skin front and sides, hair on top and at the back, two
        # eye quads. The collar hides the neck.
        x0, x1, y0, y1, z0, z1 = -0.11, 0.11, 0.06, 0.33, -0.12, 0.12
        m.poly([(x0, y0, z1), (x1, y0, z1), (x1, y1, z1), (x0, y1, z1)], SKIN, out=(0, 0, 1))
        m.poly([(x0, y0, z0), (x0, y0, z1), (x0, y1, z1), (x0, y1, z0)], shade(SKIN, 0.86), out=(-1, 0, 0))
        m.poly([(x1, y0, z1), (x1, y0, z0), (x1, y1, z0), (x1, y1, z1)], shade(SKIN, 0.86), out=(1, 0, 0))
        m.poly([(x1, y0, z0), (x0, y0, z0), (x0, y1, z0), (x1, y1, z0)], HAIR, out=(0, 0, -1))
        m.poly([(x0, y1, z1), (x1, y1, z1), (x1, y1, z0), (x0, y1, z0)], shade(HAIR, 1.1), out=(0, 1, 0))
        for x in (-0.055, 0.055):
            m.poly([(x - 0.025, 0.2, 0.121), (x + 0.025, 0.2, 0.121), (x + 0.025, 0.225, 0.121),
                    (x - 0.025, 0.225, 0.121)], EYE, out=(0, 0, 1))
    return m


def part_torso(detail):
    """Shirt on the waist pivot: tapered chest, short sleeves at the
    shoulders, a darker collar."""
    m = Model("torso")
    if not detail:
        # Consoles: one tapered block (shoulders wide enough to read as
        # sleeves) and a darker collar on top.
        m.taper(0.0, 0.46, 0.31, 0.2, 0.5, 0.24, KIT, skip=("-y", "+y"))
        m.poly([(-0.25, 0.46, 0.12), (0.25, 0.46, 0.12), (0.25, 0.46, -0.12), (-0.25, 0.46, -0.12)],
               shade(KIT, 0.62), out=(0, 1, 0))
        return m
    m.taper(0.0, 0.3, 0.31, 0.2, 0.43, 0.25, KIT, skip=("-y", "+y"))
    m.taper(0.3, 0.46, 0.43, 0.25, 0.4, 0.22, KIT, skip=("-y",))
    for s in (-1, 1):
        m.box((s * 0.25, 0.38, 0), (0.11, 0.14, 0.15), shade(KIT, 0.96), skip=("-x" if s > 0 else "+x",))
    m.box((0, 0.465, 0.0), (0.16, 0.02, 0.12), shade(KIT, 0.62), skip=("-y",))
    # Number panel on the back and a chest badge area, a touch darker.
    m.poly([(-0.08, 0.17, -0.126), (0.08, 0.17, -0.126), (0.08, 0.33, -0.126), (-0.08, 0.33, -0.126)],
           shade(KIT, 0.7), out=(0, 0, -1))
    m.poly([(0.06, 0.32, 0.132), (0.11, 0.32, 0.132), (0.11, 0.37, 0.132), (0.06, 0.37, 0.132)],
           shade(KIT, 0.7), out=(0, 0, 1))
    return m


def part_shorts(detail):
    """Shorts on the hip pivot, with the blob shadow on the ground (the
    pelvis never rolls, so it stays flat; drawn 4 cm over the turf)."""
    m = Model("shorts")
    shadow = (36, 70, 30)
    if not detail:
        m.taper(-0.3, 0.04, 0.36, 0.22, 0.31, 0.2, KIT, skip=("+y", "-y"))
        m.disc((0, 0, 0.02), 0.42, 0.32, shadow, n=8, y=-0.95 + 0.04)
        return m
    m.taper(-0.18, 0.04, 0.33, 0.21, 0.31, 0.2, KIT, skip=("+y",))
    for s in (-1, 1):
        m.taper(-0.34, -0.18, 0.15, 0.17, 0.165, 0.2, shade(KIT, 0.95), cx=s * 0.09, skip=("+y",))
    m.disc((0, 0, 0.02), 0.42, 0.32, shadow, n=10, y=-0.95 + 0.04)
    return m


def part_arm(whole):
    """Arm on the shoulder pivot: the whole arm (consoles: one tapered tube
    with the hand as its cap) or the upper arm (PC, with a separate
    forearm)."""
    m = Model("arm" if whole else "upper_arm")
    if whole:
        m.taper(-0.62, 0.0, 0.07, 0.08, 0.1, 0.1, SKIN, skip=("+y",))
        return m
    m.taper(-0.3, 0.0, 0.085, 0.085, 0.1, 0.1, SKIN)
    return m


def part_forearm():
    m = Model("forearm")
    m.taper(-0.27, 0.0, 0.072, 0.072, 0.085, 0.085, SKIN, skip=("+y",))
    m.box((0, -0.31, 0.0), (0.085, 0.08, 0.095), SKIN, skip=("+y",))
    return m


def part_leg(whole):
    """Leg on the hip pivot. Consoles: one tapered piece tinted with the
    sock color, and the boot. PC: the thigh alone (skin), the shin separate."""
    m = Model("leg" if whole else "thigh")
    if whole:
        m.taper(-0.82, 0.0, 0.1, 0.11, 0.15, 0.16, KIT, skip=("+y", "-y"))
        m.box((0, -0.865, 0.04), (0.11, 0.09, 0.26), BOOT, skip=("-y", "+y", "-z"))
        m.poly([(-0.055, -0.82, -0.09), (0.055, -0.82, -0.09), (0.055, -0.82, 0.17), (-0.055, -0.82, 0.17)],
               shade(BOOT, 1.1), out=(0, 1, 0))
        return m
    m.taper(-0.43, 0.0, 0.12, 0.13, 0.15, 0.16, SKIN)
    return m


def _shin(m, y0):
    m.taper(y0 - 0.38, y0, 0.1, 0.11, 0.12, 0.13, KIT, skip=("+y",))
    m.box((0, y0 + 0.02, 0), (0.125, 0.04, 0.135), shade(KIT, 0.72), skip=("+y", "-y"))
    by = y0 - 0.38 - 0.05
    m.box((0, by, 0.04), (0.11, 0.09, 0.26), BOOT, skip=("-y",))


def part_shin():
    m = Model("shin")
    _shin(m, 0.0)
    return m


def flag_hand():
    """The linesman's flag, held at the hand (drawn at the forearm's end)."""
    m = Model("flag_hand")
    m.box((0, 0.2, 0), (0.025, 0.55, 0.025), (60, 60, 60))
    m.two_sided([(0.0, 0.47, 0.0), (0.0, 0.47, 0.32), (0.0, 0.25, 0.32), (0.0, 0.25, 0.0)], (250, 210, 30))
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
    builders = {
        "pelvis": lambda: part_shorts(True), "torso": lambda: part_torso(True),
        "head": lambda: part_head(True), "arm_l": lambda: part_arm(False), "arm_r": lambda: part_arm(False),
        "forearm_l": part_forearm, "forearm_r": part_forearm, "thigh_l": lambda: part_leg(False),
        "thigh_r": lambda: part_leg(False), "shin_l": part_shin, "shin_r": part_shin,
    }
    objs = {}
    for name, parent, piv in anim.JOINTS:
        mdl = builders[name]()
        mdl.name = "rig_" + name
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
                    o.location = Vector(T((piv[0] + root[0], piv[1] + root[1], piv[2] + root[2])))
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
reg("boards_0", lambda: ad_boards(0), SHARED, 24, ((0, 0, 0.5), 11))
reg("boards_1", lambda: ad_boards(1), SHARED, 24, ((0, 0, 0.5), 11))
reg("stand_long_a", lambda: stand_section("stand_long_a", 19.5, 7, 1, (206, 32, 41), False), SHARED, 80, ((0, 2, 4), 22))
reg("stand_long_b", lambda: stand_section("stand_long_b", 19.5, 7, 2, (28, 90, 200), False), SHARED, 80, ((0, 2, 4), 22))
reg("stand_end_a", lambda: stand_section("stand_end_a", 20.0, 6, 3, (206, 32, 41), False), SHARED, 70, ((0, 2, 4), 22))
reg("stand_end_b", lambda: stand_section("stand_end_b", 20.0, 6, 4, (28, 90, 200), False), SHARED, 70, ((0, 2, 4), 22))
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
reg("head", lambda: part_head(False), SHARED, 24, ((0, 0, 0.18), 0.5))
reg("head@pc", lambda: part_head(True), PC, 70, ((0, 0, 0.18), 0.5))
reg("torso", lambda: part_torso(False), SHARED, 12, ((0, 0, 0.25), 0.7))
reg("torso@pc", lambda: part_torso(True), PC, 60, ((0, 0, 0.25), 0.7))
reg("shorts", lambda: part_shorts(False), SHARED, 16, ((0, 0, -0.4), 1.1))
reg("shorts@pc", lambda: part_shorts(True), PC, 50, ((0, 0, -0.4), 1.1))
reg("arm", lambda: part_arm(True), SHARED, 10, ((0, 0, -0.3), 0.8))
reg("leg", lambda: part_leg(True), SHARED, 18, ((0, 0, -0.45), 1.1))
reg("upper_arm", lambda: part_arm(False), PC, 14, ((0, 0, -0.15), 0.5))
reg("forearm", part_forearm, PC, 20, ((0, 0, -0.15), 0.5))
reg("thigh", lambda: part_leg(False), PC, 14, ((0, 0, -0.2), 0.6))
reg("shin", part_shin, PC, 30, ((0, 0, -0.25), 0.7))
reg("flag_hand", flag_hand, SHARED, 18, ((0, 0, 0.25), 0.8))
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
        if only and name not in only and key not in only:
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
