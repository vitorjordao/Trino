#!/usr/bin/env python3
"""Solid Eleven's player skeleton and animation clips: the one source of truth.

Pure Python (no Blender): `solid_eleven_models.py` builds the rig and its
actions from these tables inside Blender, and this file writes the same
poses as Rust constants for the game:

    python3 tools/blender/solid_eleven_anim.py --rust examples/solid-eleven/src/render/clips_gen.rs
    python3 tools/blender/solid_eleven_anim.py --check examples/solid-eleven/src/render/clips_gen.rs

(`cargo xtask blender solid-eleven` runs both.) The game poses rigid parts
with these quaternions on every target (one code path), and
`xtask/tests/solid_eleven_clips.rs` holds the generated file to this table.

Conventions (Trino space: x = the player's left, y = up, z = forward):
- each joint's rotation is Euler degrees (rx, ry, rz) applied X, then Y,
  then Z (R = Rz·Ry·Rx, like `Mat34::from_rotation_scale_translation`),
  in the parent's frame, at the joint's pivot;
- rx > 0 tips a part's +y end toward +z: a torso leans forward, but a
  hanging leg or arm swings **back**; rz > 0 tips +y toward -x;
- `root` is the pelvis offset (dx, dy, dz) in meters from its rest height.
"""
import math, sys

# name, parent, pivot (meters, relative to the parent's pivot)
JOINTS = [
    ("pelvis", None, (0.0, 0.95, 0.0)),
    ("torso", "pelvis", (0.0, 0.02, 0.0)),
    ("head", "torso", (0.0, 0.50, 0.0)),
    ("arm_l", "torso", (0.25, 0.43, 0.0)),
    ("arm_r", "torso", (-0.25, 0.43, 0.0)),
    ("forearm_l", "arm_l", (0.0, -0.29, 0.0)),
    ("forearm_r", "arm_r", (0.0, -0.29, 0.0)),
    ("thigh_l", "pelvis", (0.10, -0.03, 0.0)),
    ("thigh_r", "pelvis", (-0.10, -0.03, 0.0)),
    ("shin_l", "thigh_l", (0.0, -0.43, 0.0)),
    ("shin_r", "thigh_r", (0.0, -0.43, 0.0)),
]
NAMES = [j[0] for j in JOINTS]
MIRROR = {"arm_l": "arm_r", "arm_r": "arm_l", "forearm_l": "forearm_r", "forearm_r": "forearm_l",
          "thigh_l": "thigh_r", "thigh_r": "thigh_l", "shin_l": "shin_r", "shin_r": "shin_l"}


def mirror_pose(pose):
    """Left-right mirror of a pose dict: swap sides, negate ry and rz."""
    out = {}
    for j, (rx, ry, rz) in pose.items():
        out[MIRROR.get(j, j)] = (rx, -ry, -rz)
    return out


def run_keys(stride, knee, arm, elbow, lean, bob, lift):
    """A run/jog/sprint cycle: contact, passing, opposite contact, passing.
    `stride` thigh swing (deg), `knee` back-leg knee bend, `arm` arm swing,
    `elbow` bend, `lean` torso lean, `bob` pelvis rise, `lift` swing-leg knee
    lift at passing."""
    contact = {
        "torso": (lean, 4, 0), "head": (-lean * 0.6, -3, 0),
        "thigh_l": (-stride, 0, 0), "shin_l": (12, 0, 0),
        "thigh_r": (stride * 0.8, 0, 0), "shin_r": (knee, 0, 0),
        "arm_l": (arm, 0, 6), "forearm_l": (-elbow * 0.7, 0, 0),
        "arm_r": (-arm, 0, -6), "forearm_r": (-elbow, 0, 0),
    }
    passing = {
        "torso": (lean, 0, 0), "head": (-lean * 0.6, 0, 0),
        "thigh_l": (0, 0, 0), "shin_l": (8, 0, 0),
        "thigh_r": (-lift, 0, 0), "shin_r": (knee + 20, 0, 0),
        "arm_l": (0, 0, 6), "forearm_l": (-elbow, 0, 0),
        "arm_r": (0, 0, -6), "forearm_r": (-elbow, 0, 0),
    }
    return [
        (0.0, contact, (0, -bob * 0.5, 0)),
        (0.25, passing, (0, bob, 0)),
        (0.5, mirror_pose(contact), (0, -bob * 0.5, 0)),
        (0.75, mirror_pose(passing), (0, bob, 0)),
    ]


IDLE = {"torso": (2, 0, 0), "arm_l": (0, 0, 8), "arm_r": (0, 0, -8), "forearm_l": (-12, 0, 0),
        "forearm_r": (-12, 0, 0), "shin_l": (4, 0, 0), "shin_r": (4, 0, 0), "thigh_l": (-2, 0, 3),
        "thigh_r": (-2, 0, -3)}

# clip name -> (seconds, loop, keys [(t 0..1, {joint: (rx, ry, rz)}, root (dx, dy, dz))])
CLIPS = {
    "idle": (1.6, True, [
        (0.0, IDLE, (0, 0, 0)),
        (0.5, {**IDLE, "torso": (4, 0, 0), "arm_l": (0, 0, 10), "arm_r": (0, 0, -10)}, (0, -0.015, 0)),
    ]),
    "walk": (1.0, True, run_keys(stride=22, knee=25, arm=18, elbow=15, lean=2, bob=0.02, lift=12)),
    "jog": (0.70, True, run_keys(stride=34, knee=55, arm=30, elbow=55, lean=8, bob=0.04, lift=40)),
    "sprint": (0.52, True, run_keys(stride=48, knee=80, arm=48, elbow=80, lean=18, bob=0.06, lift=62)),
    "dribble": (0.62, True, run_keys(stride=30, knee=50, arm=26, elbow=50, lean=14, bob=0.035, lift=34)),
    # Kicks: plant the left foot, swing the right leg through the ball.
    "pass": (0.42, False, [
        (0.0, {"torso": (6, 0, 0), "thigh_r": (25, 0, 0), "shin_r": (50, 0, 0), "arm_l": (-10, 0, 25),
               "arm_r": (15, 0, -20), "forearm_l": (-20, 0, 0), "forearm_r": (-20, 0, 0)}, (0, 0, 0)),
        (0.45, {"torso": (4, -10, 0), "thigh_r": (-35, 15, 0), "shin_r": (5, 0, 0), "thigh_l": (5, 0, 0),
                "arm_l": (-20, 0, 35), "arm_r": (25, 0, -25), "forearm_l": (-25, 0, 0)}, (0, -0.03, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    "lob": (0.5, False, [
        (0.0, {"torso": (-4, 0, 0), "thigh_r": (40, 0, 0), "shin_r": (70, 0, 0), "arm_l": (-20, 0, 40),
               "arm_r": (20, 0, -35), "forearm_l": (-30, 0, 0)}, (0, 0, 0)),
        (0.45, {"torso": (-12, 0, 0), "thigh_r": (-60, 0, 0), "shin_r": (10, 0, 0), "arm_l": (-30, 0, 50),
                "arm_r": (30, 0, -45)}, (0, -0.04, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    "shot": (0.55, False, [
        (0.0, {"torso": (8, 0, 0), "thigh_r": (55, 0, 0), "shin_r": (95, 0, 0), "thigh_l": (-10, 0, 0),
               "arm_l": (-35, 0, 45), "arm_r": (30, 0, -40), "forearm_l": (-30, 0, 0)}, (0, 0, 0)),
        (0.35, {"torso": (14, -8, 0), "thigh_r": (-75, 10, 0), "shin_r": (8, 0, 0), "thigh_l": (8, 0, 0),
                "shin_l": (20, 0, 0), "arm_l": (-45, 0, 55), "arm_r": (40, 0, -50)}, (0, 0.05, 0.05)),
        (0.65, {"torso": (10, 0, 0), "thigh_r": (-40, 0, 0), "shin_r": (30, 0, 0), "arm_l": (-20, 0, 30),
                "arm_r": (20, 0, -30)}, (0, 0.02, 0.1)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    "tackle": (0.4, False, [
        (0.0, {"torso": (15, 0, 0), "thigh_l": (-45, 0, 8), "shin_l": (15, 0, 0), "thigh_r": (25, 0, 0),
               "shin_r": (40, 0, 0), "arm_l": (-20, 0, 25), "arm_r": (25, 0, -30)}, (0, -0.08, 0)),
        (0.5, {"torso": (22, 0, 0), "thigh_l": (-70, 0, 10), "shin_l": (5, 0, 0), "thigh_r": (30, 0, 0),
               "shin_r": (60, 0, 0), "arm_l": (-30, 0, 35), "arm_r": (30, 0, -35)}, (0, -0.15, 0.15)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # On the hip, leading leg out, the other folded: the body leans back.
    "slide": (0.6, False, [
        (0.0, {"torso": (-10, 0, 0), "thigh_l": (-60, 0, 0), "shin_l": (5, 0, 0), "thigh_r": (-20, 0, 0),
               "shin_r": (90, 0, 0), "arm_l": (-10, 0, 50), "arm_r": (10, 0, -50)}, (0, -0.4, 0)),
        (0.25, {"torso": (-45, 0, 10), "thigh_l": (-85, 0, 5), "shin_l": (0, 0, 0), "thigh_r": (-40, 0, 0),
                "shin_r": (110, 0, 0), "arm_l": (-20, 0, 70), "arm_r": (20, 0, -75)}, (0, -0.72, 0)),
        (1.0, {"torso": (-45, 0, 10), "thigh_l": (-85, 0, 5), "shin_l": (0, 0, 0), "thigh_r": (-40, 0, 0),
               "shin_r": (110, 0, 0), "arm_l": (-20, 0, 70), "arm_r": (20, 0, -75)}, (0, -0.72, 0)),
    ]),
    # Flat on the back, then up.
    "down": (1.2, False, [
        (0.0, {"torso": (-30, 0, 0), "thigh_l": (-40, 0, 0), "thigh_r": (-25, 0, 0), "arm_l": (-40, 0, 40),
               "arm_r": (-40, 0, -40)}, (0, -0.3, -0.2)),
        (0.15, {"torso": (-85, 0, 0), "head": (10, 0, 0), "thigh_l": (-80, 0, 0), "thigh_r": (-70, 0, 5),
                "shin_r": (40, 0, 0), "arm_l": (-90, 0, 50), "arm_r": (-80, 0, -60)}, (0, -0.8, -0.4)),
        (0.7, {"torso": (-85, 0, 0), "head": (10, 0, 0), "thigh_l": (-80, 0, 0), "thigh_r": (-70, 0, 5),
               "shin_r": (40, 0, 0), "arm_l": (-90, 0, 50), "arm_r": (-80, 0, -60)}, (0, -0.8, -0.4)),
        (1.0, {"torso": (25, 0, 0), "thigh_l": (-70, 0, 0), "shin_l": (110, 0, 0), "thigh_r": (-10, 0, 0),
               "shin_r": (60, 0, 0), "arm_l": (-30, 0, 20), "arm_r": (-30, 0, -20)}, (0, -0.4, 0)),
    ]),
    "header": (0.45, False, [
        (0.0, {"torso": (-10, 0, 0), "head": (-15, 0, 0), "arm_l": (-40, 0, 45), "arm_r": (-40, 0, -45),
               "thigh_l": (-10, 0, 0), "shin_l": (40, 0, 0), "thigh_r": (-10, 0, 0), "shin_r": (40, 0, 0)},
         (0, -0.1, 0)),
        (0.4, {"torso": (15, 0, 0), "head": (25, 0, 0), "arm_l": (-60, 0, 60), "arm_r": (-60, 0, -60),
               "thigh_l": (-5, 0, 0), "shin_l": (20, 0, 0), "thigh_r": (5, 0, 0), "shin_r": (30, 0, 0)},
         (0, 0.35, 0)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # Both hands over the head, then forward.
    "throw": (0.6, False, [
        (0.0, {"torso": (-15, 0, 0), "arm_l": (-170, 0, 10), "arm_r": (-170, 0, -10), "forearm_l": (-70, 0, 0),
               "forearm_r": (-70, 0, 0), "thigh_l": (-15, 0, 0)}, (0, 0, -0.05)),
        (0.4, {"torso": (12, 0, 0), "arm_l": (-120, 0, 8), "arm_r": (-120, 0, -8), "forearm_l": (-10, 0, 0),
               "forearm_r": (-10, 0, 0), "thigh_l": (-15, 0, 0)}, (0, 0, 0.05)),
        (1.0, IDLE, (0, 0, 0)),
    ]),
    # Arms up, fists pumping, little hops (the title screen's huddle).
    "celebrate": (0.9, True, [
        (0.0, {"torso": (-6, 0, 0), "head": (-15, 0, 0), "arm_l": (-160, 0, 25), "arm_r": (-160, 0, -25),
               "forearm_l": (-40, 0, 0), "forearm_r": (-40, 0, 0)}, (0, 0, 0)),
        (0.5, {"torso": (-10, 0, 0), "head": (-20, 0, 0), "arm_l": (-175, 0, 15), "arm_r": (-175, 0, -15),
               "forearm_l": (-5, 0, 0), "forearm_r": (-5, 0, 0), "thigh_l": (-25, 0, 0), "shin_l": (50, 0, 0),
               "thigh_r": (-25, 0, 0), "shin_r": (50, 0, 0)}, (0, 0.18, 0)),
    ]),
    # Keeper: ready crouch, dive (to his left; the game mirrors it), hold.
    "gk_ready": (1.2, True, [
        (0.0, {"torso": (22, 0, 0), "head": (-18, 0, 0), "thigh_l": (-35, 0, 12), "shin_l": (60, 0, 0),
               "thigh_r": (-35, 0, -12), "shin_r": (60, 0, 0), "arm_l": (-35, 0, 35), "arm_r": (-35, 0, -35),
               "forearm_l": (-30, 0, 0), "forearm_r": (-30, 0, 0)}, (0, -0.16, 0)),
        (0.5, {"torso": (24, 0, 0), "head": (-18, 0, 0), "thigh_l": (-38, 0, 12), "shin_l": (64, 0, 0),
               "thigh_r": (-38, 0, -12), "shin_r": (64, 0, 0), "arm_l": (-40, 0, 38), "arm_r": (-40, 0, -38),
               "forearm_l": (-30, 0, 0), "forearm_r": (-30, 0, 0)}, (0, -0.18, 0)),
    ]),
    "gk_dive": (0.6, False, [
        (0.0, {"torso": (10, 0, -20), "thigh_l": (-30, 0, 20), "shin_l": (50, 0, 0), "thigh_r": (-20, 0, 0),
               "arm_l": (-60, 0, 60), "arm_r": (-60, 0, 20)}, (0, -0.15, 0)),
        (0.3, {"torso": (0, 0, -75), "head": (0, 0, -10), "thigh_l": (0, 0, 15), "thigh_r": (-10, 0, -10),
               "shin_r": (30, 0, 0), "arm_l": (-10, 0, 170), "arm_r": (-10, 0, 150)}, (0.6, 0.15, 0)),
        (0.7, {"torso": (0, 0, -88), "head": (0, 0, -10), "thigh_l": (0, 0, 10), "thigh_r": (-10, 0, -5),
               "shin_r": (30, 0, 0), "arm_l": (-10, 0, 170), "arm_r": (-10, 0, 160)}, (1.0, -0.55, 0)),
        (1.0, {"torso": (0, 0, -88), "head": (0, 0, -10), "thigh_l": (0, 0, 10), "thigh_r": (-10, 0, -5),
               "shin_r": (30, 0, 0), "arm_l": (-10, 0, 170), "arm_r": (-10, 0, 160)}, (1.1, -0.75, 0)),
    ]),
    "gk_hold": (1.2, True, [
        (0.0, {"torso": (8, 0, 0), "arm_l": (-70, 0, -15), "arm_r": (-70, 0, 15), "forearm_l": (-50, 0, 0),
               "forearm_r": (-50, 0, 0)}, (0, 0, 0)),
        (0.5, {"torso": (10, 0, 0), "arm_l": (-72, 0, -15), "arm_r": (-72, 0, 15), "forearm_l": (-52, 0, 0),
               "forearm_r": (-52, 0, 0)}, (0, -0.01, 0)),
    ]),
    # Officials: the referee's arm points the restart, a card goes up; the
    # linesman raises his flag (the flag is drawn at his hand).
    "ref_point": (0.8, False, [
        (0.0, IDLE, (0, 0, 0)),
        (0.4, {**IDLE, "arm_l": (-95, 0, 25), "forearm_l": (0, 0, 0)}, (0, 0, 0)),
        (1.0, {**IDLE, "arm_l": (-95, 0, 25), "forearm_l": (0, 0, 0)}, (0, 0, 0)),
    ]),
    "ref_card": (0.8, False, [
        (0.0, IDLE, (0, 0, 0)),
        (0.4, {**IDLE, "torso": (-4, 0, 0), "arm_r": (-178, 0, -4), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
        (1.0, {**IDLE, "torso": (-4, 0, 0), "arm_r": (-178, 0, -4), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
    ]),
    "flag_up": (0.6, False, [
        (0.0, IDLE, (0, 0, 0)),
        (0.5, {**IDLE, "arm_r": (-175, 0, -6), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
        (1.0, {**IDLE, "arm_r": (-175, 0, -6), "forearm_r": (0, 0, 0)}, (0, 0, 0)),
    ]),
}


def quat_from_euler(rx, ry, rz):
    """(x, y, z, w) of R = Rz·Ry·Rx, degrees."""
    hx, hy, hz = (math.radians(a) * 0.5 for a in (rx, ry, rz))
    cx, sx = math.cos(hx), math.sin(hx)
    cy, sy = math.cos(hy), math.sin(hy)
    cz, sz = math.cos(hz), math.sin(hz)
    # q = qz * qy * qx
    w = cz * cy * cx + sz * sy * sx
    x = cz * cy * sx - sz * sy * cx
    y = cz * sy * cx + sz * cy * sx
    z = sz * cy * cx - cz * sy * sx
    return (x, y, z, w)


def key_quats(pose):
    return [quat_from_euler(*pose.get(name, (0, 0, 0))) for name in NAMES]


def rust_source():
    out = []
    w = out.append
    w("// @generated by tools/blender/solid_eleven_anim.py — do not edit.")
    w("// `cargo xtask blender solid-eleven` regenerates it (Blender not needed for")
    w("// this file: `python3 tools/blender/solid_eleven_anim.py --rust <path>`).")
    w("")
    w("use super::anim::{Clip, Key};")
    w("")
    w(f"/// Joints, in order: {', '.join(NAMES)}.")
    w(f"pub const JOINTS: usize = {len(NAMES)};")
    w("")
    w("/// Each joint's parent (`usize::MAX` = the root) and pivot from it (m).")
    w("pub const SKELETON: [(usize, [f32; 3]); JOINTS] = [")
    for name, parent, piv in JOINTS:
        p = "usize::MAX" if parent is None else str(NAMES.index(parent))
        w(f"    ({p}, [{piv[0]:.4f}, {piv[1]:.4f}, {piv[2]:.4f}]), // {name}")
    w("];")
    w("")
    for ci, (name, (secs, loop, keys)) in enumerate(CLIPS.items()):
        w(f"const K_{name.upper()}: [Key; {len(keys)}] = [")
        for t, pose, root in keys:
            qs = ", ".join("[{:.5f}, {:.5f}, {:.5f}, {:.5f}]".format(*q) for q in key_quats(pose))
            w(f"    Key {{ t: {t:.4f}, root: [{root[0]:.4f}, {root[1]:.4f}, {root[2]:.4f}], q: [{qs}] }},")
        w("];")
    w("")
    w("/// Every clip, indexed by `ClipName as usize`.")
    w(f"pub const CLIPS: [Clip; {len(CLIPS)}] = [")
    for name, (secs, loop, keys) in CLIPS.items():
        w(f"    Clip {{ name: \"{name}\", seconds: {secs:.4f}, looped: {'true' if loop else 'false'}, keys: &K_{name.upper()} }},")
    w("];")
    w("")
    w("/// Clip names as an enum (same order as [`CLIPS`]).")
    w("#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]")
    w("pub enum ClipName {")
    for name in CLIPS:
        w(f"    {''.join(p.capitalize() for p in name.split('_'))},")
    w("}")
    return "\n".join(out) + "\n"


def main(argv):
    if len(argv) >= 2 and argv[0] in ("--rust", "--check"):
        text = rust_source()
        if argv[0] == "--rust":
            # UTF-8 and LF on every OS (Windows defaults to cp1252 and CRLF).
            with open(argv[1], "w", encoding="utf-8", newline="\n") as f:
                f.write(text)
            print(f"wrote {argv[1]}: {len(CLIPS)} clips, {len(NAMES)} joints")
            return 0
        cur = ""
        if argv[1] != "-":
            with open(argv[1], encoding="utf-8", newline="") as f:
                cur = f.read()
        if cur != text:
            print(f"{argv[1]} is stale: run python3 tools/blender/solid_eleven_anim.py --rust {argv[1]}")
            return 1
        print("clips up to date")
        return 0
    print(__doc__)
    return 2


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
