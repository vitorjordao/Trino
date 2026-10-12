# Vertex-color painting for Trino assets.
#
# Trino's 3D is vertex-colored (the N64 ceiling: no textures on 3D geometry).
# The asset bake resolves each vertex color in this priority order:
#   1. COLOR_0 (the mesh's color attribute)            <- what these tools write
#   2. else the material's base-color TEXTURE sampled at the vertex UV
#      (nearest texel — palette atlases collapse to flat colors)
#   3. else the material's base-color FACTOR
# always multiplied by the base-color factor. A 512 px painted texture does NOT
# survive: detail must live in geometry or in per-vertex/per-face colors.
#
# Load (once per session):
#   exec(open(r"<repo>/tools/blender/color_tools.py", encoding="utf-8").read(), bpy.app.driver_namespace)
#
#   P = ns["Painter"]("Body")              # works on face corners (crisp, flat colors per face)
#   P.fill((200, 120, 80))                 # base color
#   P.paint(lambda c, n: c.z > 1.6, (230, 200, 160), "face")   # rule on face center c / normal n
#   P.gradient((60, 40, 30), (110, 80, 60), axis=2)            # bottom -> top
#   P.noise(0.06)                          # subtle per-face variation (reads as hand-made)
#   P.ambient_occlusion_floor(0.25)        # darken near the ground plane (fake contact shadow)
#   P.commit()                             # writes the "Col" corner attribute and makes it active
#   ns["palette_from_materials"]("Prop")   # alternative: bake each material's base color into Col
#
# Rules that saved rework (inherited from a production character pipeline):
# - Paint by **3D position** rules, never by UV: seams stop mattering.
# - Region borders are smooth functions (z = a + b·cos φ around a neck, ellipsoids for
#   faces), not straight cuts through the middle of a face.
# - "Front/side" by **angle around the body axis** (atan2), not by face normal — flat-
#   shaded normals make jagged borders.
# - Think of the object, not of each concept view: a mouth, a stripe or a belt is one
#   continuous loop around the body. Check front, side AND from below.
# - Small detail that does not fit as color needs geometry (an extra edge loop), not a
#   texture. Budget: keep props tens to a few hundred triangles; see `cargo xtask model inspect`.
#
# STATUS: written for Blender 4.2+/5.x; not executed in this repository's CI.
import bpy, math
import numpy as np


def _srgb_to_linear(c):
    c = np.asarray(c, dtype=np.float64) / 255.0
    return np.where(c <= 0.04045, c / 12.92, ((c + 0.055) / 1.055) ** 2.4)


class Painter:
    """Per-face-corner colors (FLOAT/BYTE color on CORNER domain). Each face gets
    one color unless you blend explicitly — the faceted look Trino's Flat shading
    is built for. glTF exports corner colors by splitting vertices as needed."""

    def __init__(self, obj_name):
        self.obj = bpy.data.objects[obj_name]
        me = self.obj.data
        self.me = me
        n = len(me.polygons)
        self.centers = np.array([p.center[:] for p in me.polygons])
        self.normals = np.array([p.normal[:] for p in me.polygons])
        self.colors = np.ones((n, 3)) * 0.8   # linear RGB per face
        self.lo = self.centers.min(0) if n else np.zeros(3)
        self.hi = self.centers.max(0) if n else np.ones(3)

    def fill(self, rgb):
        self.colors[:] = _srgb_to_linear(rgb)

    def paint(self, rule, rgb, _label=""):
        """rule(center: Vector-like, normal) -> bool, evaluated per face."""
        col = _srgb_to_linear(rgb)
        for i, (c, n) in enumerate(zip(self.centers, self.normals)):
            if rule(_V(c), _V(n)):
                self.colors[i] = col

    def gradient(self, bottom_rgb, top_rgb, axis=2):
        a, b = _srgb_to_linear(bottom_rgb), _srgb_to_linear(top_rgb)
        span = max(self.hi[axis] - self.lo[axis], 1e-6)
        t = ((self.centers[:, axis] - self.lo[axis]) / span).clip(0, 1)[:, None]
        self.colors[:] = a * (1 - t) + b * t

    def noise(self, amount=0.05, seed=7):
        rng = np.random.default_rng(seed)
        self.colors *= 1.0 + rng.uniform(-amount, amount, (len(self.colors), 1))

    def ambient_occlusion_floor(self, strength=0.25, height=0.3):
        z = self.centers[:, 2] - self.lo[2]
        k = 1.0 - strength * (1.0 - (z / max(height, 1e-6)).clip(0, 1))
        self.colors *= k[:, None]

    def commit(self, name="Col"):
        me = self.me
        attr = me.color_attributes.get(name)
        if attr is None or attr.domain != 'CORNER':
            if attr is not None:
                me.color_attributes.remove(attr)
            attr = me.color_attributes.new(name=name, type='BYTE_COLOR', domain='CORNER')
        per_loop = np.zeros((len(me.loops), 4))
        for p in me.polygons:
            c = self.colors[p.index].clip(0, 1)
            for li in p.loop_indices:
                per_loop[li, :3] = c
                per_loop[li, 3] = 1.0
        attr.data.foreach_set("color", per_loop.ravel())
        me.color_attributes.active_color = attr
        try:
            me.color_attributes.render_color_index = me.color_attributes.active_color_index
        except AttributeError:
            pass
        me.update()
        return {"faces": len(me.polygons), "attribute": name}


class _V:
    """Tiny vector view (.x .y .z) over a numpy row for rule lambdas."""

    __slots__ = ("x", "y", "z")

    def __init__(self, a):
        self.x, self.y, self.z = float(a[0]), float(a[1]), float(a[2])


def palette_from_materials(obj_name, name="Col"):
    """Bake each face's material base color into the color attribute — turns a
    multi-material low-poly model (Kenney/Quaternius style) into one mesh with
    vertex colors, no textures needed."""
    obj = bpy.data.objects[obj_name]
    me = obj.data

    def base(mat):
        if mat is None:
            return (0.8, 0.8, 0.8)
        if mat.use_nodes and mat.node_tree:
            for node in mat.node_tree.nodes:
                if node.type == 'BSDF_PRINCIPLED':
                    return tuple(node.inputs["Base Color"].default_value[:3])
        return tuple(mat.diffuse_color[:3])

    P = Painter(obj_name)
    for p in me.polygons:
        mat = obj.material_slots[p.material_index].material if obj.material_slots else None
        P.colors[p.index] = base(mat)  # already linear
    return P.commit(name)


def report(obj_name):
    """Quick numbers before export: triangles, color attribute, materials."""
    obj = bpy.data.objects[obj_name]
    me = obj.data
    tris = sum(len(p.vertices) - 2 for p in me.polygons)
    return {
        "triangles": tris,
        "vertices": len(me.vertices),
        "color_attributes": [a.name for a in me.color_attributes],
        "active_color": me.color_attributes.active_color.name if me.color_attributes.active_color else None,
        "materials": [s.material.name if s.material else None for s in obj.material_slots],
    }
