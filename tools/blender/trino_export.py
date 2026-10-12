# Export from Blender to Trino (glTF binary) and verify with the engine's own bake.
#
# Load: exec(open(r"<repo>/tools/blender/trino_export.py", encoding="utf-8").read(), bpy.app.driver_namespace)
#
#   ns["REPO"] = r"/path/to/Trino"                          # once
#   ns["stash_clips"](["walk", "idle"], ["body", "arm_l", "arm_r"])   # rigid parts: one NLA track per clip
#   ns["export_glb"](["Robot"], "assets/shared/models/robot.glb")     # root(s) + children
#   ns["inspect"]("assets/shared/models/robot.glb", fps=20, once=["wave"])  # the acceptance gate
#
# Conventions (verified against the engine, not the exporter):
# - glTF is Y-up; the exporter converts Blender's Z-up (keep its +Y-up option on).
#   1 Blender unit = 1 m = 1 Trino unit. Apply scale/rotation on objects that should
#   not animate (Ctrl+A); animated parts keep their transforms (they ARE the joints).
# - Origin of the root at the base (feet / bottom center): scenes place models on y = 0.
#   `inspect` warns otherwise.
# - Vertex colors: export the active color attribute (see color_tools.py for priority).
# - Animations: rigid-part models export one animation per NLA track name (clip),
#   merging every object's strip with that name. Armature models: one action per clip.
#
# Exporter option names changed across Blender versions; `_supported` drops keys the
# running exporter does not know and returns them so you can see what was skipped.
# STATUS: not executed in this repository's CI — `inspect` is the gate that matters.
import bpy, json, os, subprocess

REPO = globals().get("REPO")


def _supported(op, wanted):
    try:
        props = {p.identifier for p in op.get_rna_type().properties}
    except Exception:
        return wanted, []
    keep = {k: v for k, v in wanted.items() if k in props}
    return keep, sorted(set(wanted) - set(keep))


def stash_clips(clips, objects):
    """For each clip name, push the actions named '<clip>__<object>' (what
    rig_tools.make_clip creates) into an NLA track named '<clip>' on each object.
    Tracks are left muted and the active action cleared so the viewport shows the
    rest pose; the exporter still sees every track."""
    done = []
    for obj_name in objects:
        o = bpy.data.objects[obj_name]
        ad = o.animation_data_create()
        ad.action = None
        for t in list(ad.nla_tracks):
            if t.name in clips:
                ad.nla_tracks.remove(t)
        for clip in clips:
            act = bpy.data.actions.get(f"{clip}__{obj_name}")
            if act is None:
                continue
            tr = ad.nla_tracks.new()
            tr.name = clip
            st = tr.strips.new(clip, int(act.frame_range[0]), act)
            st.name = clip
            try:
                st.action_slot = act.slots[0]
            except Exception:
                pass
            tr.mute = True
            done.append((obj_name, clip))
    return done


def _select_tree(roots):
    for ob in bpy.context.selected_objects:
        ob.select_set(False)
    picked = []

    def walk(o):
        o.select_set(True)
        picked.append(o.name)
        for c in o.children:
            walk(c)

    for r in roots:
        walk(bpy.data.objects[r])
    bpy.context.view_layer.objects.active = bpy.data.objects[roots[0]]
    return picked


def export_glb(roots, out_path, animations=True, nla=True):
    """Export the given root objects (and their children) to a .glb.
    out_path may be relative to REPO."""
    if REPO and not os.path.isabs(out_path):
        out_path = os.path.join(REPO, out_path)
    os.makedirs(os.path.dirname(out_path), exist_ok=True)
    picked = _select_tree(roots)
    # Unmute NLA tracks for the export (muted tracks are skipped by some versions).
    muted = []
    for name in picked:
        ad = bpy.data.objects[name].animation_data
        if ad:
            for t in ad.nla_tracks:
                if t.mute:
                    t.mute = False
                    muted.append(t)
    wanted = dict(
        filepath=out_path,
        export_format='GLB',
        use_selection=True,
        export_yup=True,
        export_apply=True,               # apply modifiers (Mirror, Bevel...)
        export_normals=True,
        export_texcoords=True,
        export_materials='EXPORT',
        export_vertex_color='ACTIVE',    # Blender 4.2+
        export_colors=True,              # older exporters
        export_animations=animations,
        export_animation_mode='NLA_TRACKS' if nla else 'ACTIONS',
        export_nla_strips=nla,           # older exporters
        export_force_sampling=True,
        export_frame_step=1,
        export_skins=True,
        export_morph=False,
        export_cameras=False,
        export_lights=False,
    )
    kwargs, skipped = _supported(bpy.ops.export_scene.gltf, wanted)
    try:
        bpy.ops.export_scene.gltf(**kwargs)
    finally:
        for t in muted:
            t.mute = True
    return {"path": out_path, "objects": picked, "skipped_options": skipped}


def inspect(path, fps=None, once=None):
    """Run the engine's bake on the exported file (`cargo xtask model inspect --json`)
    and return the report: triangles, joints, clip names/durations, bounds, warnings.
    This replaces any re-import check: it is exactly what PC, N64 and 3DS will load."""
    repo = REPO or os.getcwd()
    if not os.path.isabs(path):
        path = os.path.join(repo, path)
    cmd = ["cargo", "xtask", "model", "inspect", path, "--json"]
    if fps:
        cmd += ["--fps", str(fps)]
    if once:
        cmd += ["--once", ",".join(once)]
    p = subprocess.run(cmd, cwd=repo, capture_output=True, text=True)
    line = (p.stdout.strip().splitlines() or ["{}"])[-1]
    try:
        report = json.loads(line)
    except json.JSONDecodeError:
        report = {"ok": False, "error": p.stderr[-2000:] or p.stdout[-2000:]}
    return report
