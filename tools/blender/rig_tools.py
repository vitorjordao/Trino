# Rigging + animation helpers for Trino's **rigid skeleton** model.
#
# What Trino bakes (crates/asset-pipeline/src/model.rs):
# - Each vertex follows exactly ONE joint. With a skin, the heaviest weight wins
#   (blended weights are snapped — joints crack instead of bending). Without a skin,
#   every scene node becomes a joint and its mesh follows it.
# - At most 32 joints per model; one skin per file.
# - Animations: node translation / rotation / scale. No morph targets (shape keys).
#   CUBICSPLINE tangents are dropped (LINEAR is what you want anyway).
# - Clips are sampled at the manifest's `anim_fps` (default 15) and loop unless
#   listed in `once = [...]`.
#
# Two ways to build an animated object, both supported:
#   A. RIGID PARTS (simplest, N64-authentic): one mesh object per moving part, parented
#      in a hierarchy, origin at the part's pivot (shoulder, hinge, axle). Animate the
#      OBJECTS. A static prop with one moving part is just two objects.
#   B. ARMATURE: one mesh skinned to bones with weight 1.0 per vertex (use snap_rigid
#      to preview exactly what Trino will do). Handy when you already have a rig.
#
# Load: exec(open(r"<repo>/tools/blender/rig_tools.py", encoding="utf-8").read(), bpy.app.driver_namespace)
#
#   ns["part_report"]("Robot")                  # hierarchy, pivots, joint count vs 32
#   ns["set_pivot"]("arm_l", (0.45, 0, 1.35))   # move an object's origin without moving its mesh
#   ns["key_rot"]("arm_l", frame, [("X", 30)])  # rotation key in the PARENT's frame
#   ns["make_clip"]("walk", objs, length, keys) # one action per clip, ready for export
#   ns["rigid_check"]("Body")                   # vertices whose top weight < 0.99 (will crack)
#   ns["snap_rigid"]("Body")                    # set every vertex to its heaviest group = Trino's result
#
# STATUS: written for Blender 4.2+/5.x (actions with slots handled); not executed
# in this repository's CI. Verify with `cargo xtask model inspect` after export.
import bpy, math
from mathutils import Matrix, Vector

AX = {"X": Vector((1, 0, 0)), "Y": Vector((0, 1, 0)), "Z": Vector((0, 0, 1))}
MAX_JOINTS = 32


def rot_matrix(rots):
    """[(axis, degrees), ...] applied in order (axis = 'X'/'Y'/'Z' or a vector)."""
    R = Matrix.Identity(3)
    for ax, deg in rots:
        R = Matrix.Rotation(math.radians(deg), 3, AX[ax] if isinstance(ax, str) else Vector(ax)) @ R
    return R


def part_report(root_name):
    """Hierarchy under root: joints Trino will create for an animated rigid-part
    model (every node), plus pivots and triangle counts."""
    root = bpy.data.objects[root_name]
    rows = []

    def walk(o, depth):
        tris = 0
        if o.type == 'MESH':
            tris = sum(len(p.vertices) - 2 for p in o.data.polygons)
        rows.append({"name": o.name, "depth": depth, "type": o.type, "tris": tris,
                     "pivot": tuple(round(v, 3) for v in o.matrix_world.translation)})
        for c in o.children:
            walk(c, depth + 1)

    walk(root, 0)
    n = len(rows)
    return {
        "nodes": rows,
        "joints_if_animated": n,
        "ok": n <= MAX_JOINTS,
        "hint": None if n <= MAX_JOINTS else "join static parts into their parent (Ctrl+J) — Trino allows 32 joints",
    }


def set_pivot(obj_name, world_point):
    """Move an object's origin to world_point keeping the mesh where it is
    (the joint pivot for a rigid part)."""
    o = bpy.data.objects[obj_name]
    p = Vector(world_point)
    local = o.matrix_world.inverted() @ p
    o.data.transform(Matrix.Translation(-local))
    o.matrix_world = o.matrix_world @ Matrix.Translation(local)
    return tuple(o.matrix_world.translation)


def _set_action(obj, action):
    obj.animation_data_create()
    obj.animation_data.action = action
    if action is not None and getattr(action, "slots", None):
        try:
            obj.animation_data.action_slot = action.slots[0]
        except Exception:
            pass


def key_rot(obj_name, frame, rots, rest=None):
    """Keyframe a rotation (quaternion) relative to the object's rest rotation,
    expressed in the parent's frame — 'rotate arm_l 30° around X'."""
    o = bpy.data.objects[obj_name]
    base = rest if rest is not None else o.get("_rest_q")
    if base is None:
        o["_rest_q"] = list(o.matrix_basis.to_quaternion())
        base = o["_rest_q"]
    from mathutils import Quaternion
    q = rot_matrix(rots).to_quaternion() @ Quaternion(base)
    o.rotation_mode = 'QUATERNION'
    o.rotation_quaternion = q
    o.keyframe_insert("rotation_quaternion", frame=frame)


def key_loc(obj_name, frame, offset):
    o = bpy.data.objects[obj_name]
    base = o.get("_rest_t")
    if base is None:
        o["_rest_t"] = list(o.location)
        base = o["_rest_t"]
    o.location = Vector(base) + Vector(offset)
    o.keyframe_insert("location", frame=frame)


def make_clip(name, objects, length, keys, loop=True):
    """Create one action per object named '<name>' and key it.
    keys: list of (frame, {obj_name: [(axis, deg), ...]}). For loops the last key
    repeats frame 0 so the seam is invisible. Returns the actions.

    glTF export groups actions into animations by NLA track/strip name; see
    trino_export.stash_clips — run it after creating every clip."""
    actions = []
    for name_obj in objects:
        o = bpy.data.objects[name_obj]
        act = bpy.data.actions.get(f"{name}__{name_obj}") or bpy.data.actions.new(f"{name}__{name_obj}")
        act.use_fake_user = True
        _set_action(o, act)
        actions.append(act)
    for frame, pose in keys:
        for obj_name, rots in pose.items():
            key_rot(obj_name, frame, rots)
    if loop and keys:
        f0, p0 = keys[0]
        for obj_name, rots in p0.items():
            key_rot(obj_name, f0 + length, rots)
    for act in actions:
        for fc in _fcurves(act):
            for kp in fc.keyframe_points:
                kp.interpolation = 'LINEAR'
    bpy.context.scene.frame_end = max(bpy.context.scene.frame_end, int(length))
    return actions


def _fcurves(action):
    # Blender 5 layered actions keep fcurves in channelbags; older ones on the action.
    if hasattr(action, "fcurves"):
        return list(action.fcurves)
    out = []
    for layer in getattr(action, "layers", []):
        for strip in layer.strips:
            for bag in getattr(strip, "channelbags", []):
                out.extend(bag.fcurves)
    return out


def rigid_check(mesh_name, threshold=0.99):
    """Vertices whose strongest group weight is below threshold — Trino snaps
    them to that group, so blended joints crack. Fix in Blender (or snap_rigid)."""
    o = bpy.data.objects[mesh_name]
    bad = []
    for v in o.data.vertices:
        ws = sorted((g.weight for g in v.groups), reverse=True)
        top = ws[0] if ws else 0.0
        if top < threshold:
            bad.append(v.index)
    return {"vertices": len(o.data.vertices), "blended": len(bad), "sample": bad[:20]}


def snap_rigid(mesh_name):
    """Make every vertex weight 1.0 on its heaviest group (what the Trino bake does),
    so Blender previews the in-game deformation exactly."""
    o = bpy.data.objects[mesh_name]
    groups = o.vertex_groups
    for v in o.data.vertices:
        if not v.groups:
            continue
        best = max(v.groups, key=lambda g: g.weight).group
        for g in list(v.groups):
            groups[g.group].remove([v.index])
        groups[best].add([v.index], 1.0, 'REPLACE')
    o.data.update()
    return rigid_check(mesh_name)


def bake_to_deform(arm, ctrl_action, out_name, frame_end, frame_start=0):
    """Visual bake of a control action (IK, constraints) onto the deform bones —
    glTF exports plain bone transforms. Mute constraints afterwards so the IK does
    not keep driving the pose."""
    _set_action(arm, ctrl_action)
    sc = bpy.context.scene
    sc.frame_start, sc.frame_end = frame_start, frame_end
    for ob in bpy.context.selected_objects:
        ob.select_set(False)
    bpy.context.view_layer.objects.active = arm
    arm.select_set(True)
    bpy.ops.object.mode_set(mode='POSE')
    for pb in arm.pose.bones:
        sel = pb.bone.use_deform
        if hasattr(pb, "select"):
            pb.select = sel
        else:
            pb.bone.select = sel
    old = bpy.data.actions.get(out_name)
    if old:
        bpy.data.actions.remove(old)
    bpy.ops.nla.bake(frame_start=frame_start, frame_end=frame_end, step=1, only_selected=True,
                     visual_keying=True, clear_constraints=False, clear_parents=False,
                     use_current_action=False, clean_curves=False, bake_types={'POSE'},
                     channel_types={'LOCATION', 'ROTATION'})
    baked = arm.animation_data.action
    baked.name = out_name
    baked.use_fake_user = True
    bpy.ops.object.mode_set(mode='OBJECT')
    for pb in arm.pose.bones:
        for c in pb.constraints:
            c.mute = True
    return baked
