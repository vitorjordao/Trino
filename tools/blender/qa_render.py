# QA renders for assets in Blender (driven by an AI agent through a Blender MCP
# server, or by hand from Blender's Python console).
#
# Load once per session — functions live in bpy.app.driver_namespace, which
# survives between MCP calls:
#   import bpy; exec(open(r"<repo>/tools/blender/qa_render.py", encoding="utf-8").read(),
#                    bpy.app.driver_namespace)
# Then: ns = bpy.app.driver_namespace; ns["turntable"]("turn.png", (0, 0, 1), 3.0)
#
#   shot(path, az, el, target, ortho)    -> numpy image (orthographic camera; az 0 = from -Y)
#   sheet(path, tiles, cols)             -> contact sheet PNG (open it with an image viewer / Read tool)
#   turntable(out, target, ortho)        -> 8 azimuths + top/bottom views: the minimum before
#                                           calling a mesh or its colors "done"
#   clip_sheet(obj, action, frames, views, out) -> frames of an action, one row per view
#   orient_mode(True/False, name)        -> renderable "Face Orientation" (blue = front,
#                                           red = back faces) in EEVEE
#   vertex_color_mode(True/False)        -> Workbench showing the vertex colors Trino will use
#
# STATUS: adapted from a production pipeline; not executed in this repository's
# CI (no Blender there). If Blender's API moved, fix this file and note it in
# .agents/skills/blender-asset/SKILL.md.
#
# Lessons that shaped this file:
# - Compose images inside Blender with numpy (bpy.data.images + pixels): some agent
#   machines have no Python/PIL outside Blender.
# - Workbench has no Face Orientation; the QA_Orient material uses
#   Geometry.Backfacing + Emission (needs no lights).
# - Empties and armatures are hidden from QA renders automatically.
import bpy, math, os, tempfile
import numpy as np
import mathutils

SCRATCH = globals().get("SCRATCH") or tempfile.gettempdir()


def _cam():
    sc = bpy.context.scene
    cam = bpy.data.objects.get("QA_Cam")
    if cam is None:
        cam = bpy.data.objects.new("QA_Cam", bpy.data.cameras.new("QA_Cam"))
        sc.collection.objects.link(cam)
    cam.data.type = 'ORTHO'
    cam.data.clip_end = 500
    sc.camera = cam
    return cam


def shot(path, az, el, target, ortho, res=360, hide=()):
    """Orthographic render. az in degrees around Z (0 = from -Y, 90 = from +X,
    180 = from +Y), el = elevation in degrees. Blender is Z-up; Trino's glTF
    export converts to Y-up, so 'front' here is whatever faces -Y in Blender."""
    sc = bpy.context.scene
    cam = _cam()
    cam.data.ortho_scale = ortho
    t = mathutils.Vector(target)
    d = mathutils.Vector((math.cos(math.radians(el)) * math.sin(math.radians(az)),
                          -math.cos(math.radians(el)) * math.cos(math.radians(az)),
                          math.sin(math.radians(el))))
    cam.location = t + d * 60
    cam.rotation_euler = (-d).to_track_quat('-Z', 'Y').to_euler()
    sc.render.resolution_x = sc.render.resolution_y = res
    sc.render.resolution_percentage = 100
    sc.render.filepath = path
    sc.render.image_settings.file_format = 'PNG'
    hidden = []
    for o in bpy.data.objects:
        if (o.type in ('EMPTY', 'ARMATURE', 'CAMERA', 'LIGHT') or o.name in hide) and not o.hide_render:
            if o.name == "QA_Cam":
                continue
            o.hide_render = True
            hidden.append(o)
    bpy.ops.render.render(write_still=True)
    for o in hidden:
        o.hide_render = False
    img = bpy.data.images.load(path, check_existing=False)
    a = np.array(img.pixels[:], dtype=np.float32).reshape(res, res, 4)[::-1, :, :3]
    bpy.data.images.remove(img)
    return a


def sheet(path, tiles, cols):
    h, w, _ = tiles[0].shape
    rows = (len(tiles) + cols - 1) // cols
    out = np.full((rows * h, cols * w, 3), 0.2, np.float32)
    for i, t in enumerate(tiles):
        r, c = divmod(i, cols)
        out[r * h:(r + 1) * h, c * w:(c + 1) * w] = t
    img = bpy.data.images.new("qa_sheet", cols * w, rows * h)
    img.pixels[:] = np.concatenate([out[::-1], np.ones((rows * h, cols * w, 1), np.float32)], 2).ravel()
    img.filepath_raw = path
    img.file_format = 'PNG'
    img.save()
    bpy.data.images.remove(img)
    return path


def set_action(obj, action):
    """Assign an action (Blender 4.4+/5.x actions have slots)."""
    obj.animation_data_create()
    obj.animation_data.action = action
    if action is not None and getattr(action, "slots", None):
        obj.animation_data.action_slot = action.slots[0]


def clip_sheet(obj, action, frames, views, out, target=(0, 0, 1), ortho=3.0, res=220):
    set_action(obj, action)
    tiles = []
    for az, el in views:
        for f in frames:
            bpy.context.scene.frame_set(int(f), subframe=float(f) - int(f))
            tiles.append(shot(os.path.join(SCRATCH, "_qa_tile.png"), az, el, target, ortho, res=res))
    return sheet(os.path.join(SCRATCH, out), tiles, len(frames))


def vertex_color_mode(on=True):
    """Workbench showing the color attribute — what Trino bakes when the mesh
    has COLOR_0 (see color_tools.py for the priority rules)."""
    sc = bpy.context.scene
    sc.render.engine = 'BLENDER_WORKBENCH'
    sc.display.shading.light = 'STUDIO'
    sc.display.shading.color_type = 'VERTEX' if on else 'MATERIAL'


def _orient_material():
    m = bpy.data.materials.get("QA_Orient")
    if m:
        return m
    m = bpy.data.materials.new("QA_Orient")
    m.use_nodes = True
    nt = m.node_tree
    nt.nodes.clear()
    out = nt.nodes.new("ShaderNodeOutputMaterial")
    em = nt.nodes.new("ShaderNodeEmission")
    geo = nt.nodes.new("ShaderNodeNewGeometry")
    mix = nt.nodes.new("ShaderNodeMix")
    mix.data_type = 'RGBA'
    mix.inputs[6].default_value = (0.15, 0.3, 1, 1)  # A: front
    mix.inputs[7].default_value = (1, 0.05, 0.05, 1)  # B: back
    nt.links.new(geo.outputs["Backfacing"], mix.inputs[0])
    nt.links.new(mix.outputs[2], em.inputs[0])
    nt.links.new(em.outputs[0], out.inputs[0])
    m.use_backface_culling = False
    return m


_ORIENT = {}


def orient_mode(on, name):
    """Render every face blue (front) / red (back). Trino culls back faces, so any
    red seen from outside is a hole in the game."""
    sc = bpy.context.scene
    obj = bpy.data.objects[name]
    if on:
        _ORIENT[name] = list(obj.data.materials)
        _ORIENT["engine"] = sc.render.engine
        m = _orient_material()
        if not obj.data.materials:
            obj.data.materials.append(m)
        for i in range(len(obj.data.materials)):
            obj.data.materials[i] = m
        for e in ("BLENDER_EEVEE_NEXT", "BLENDER_EEVEE"):
            try:
                sc.render.engine = e
                break
            except TypeError:
                pass
    elif name in _ORIENT:
        mats = _ORIENT.pop(name)
        obj.data.materials.clear()
        for m in mats:
            obj.data.materials.append(m)
        sc.render.engine = _ORIENT.get("engine", sc.render.engine)


def turntable(out, target, ortho, els=(10, 60, -40), azs=(0, 45, 90, 135, 180, 225, 270, 315), res=260):
    """Full turn + top/bottom views."""
    tiles = [shot(os.path.join(SCRATCH, "_qa_tile.png"), az, els[0], target, ortho, res=res) for az in azs]
    for el in els[1:]:
        tiles += [shot(os.path.join(SCRATCH, "_qa_tile.png"), az, el, target, ortho, res=res) for az in (30, 210)]
    return sheet(os.path.join(SCRATCH, out), tiles, 4)
